//! Broker 后台部署与服务重启操作。

use std::{
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    bindings::CREATE_NO_WINDOW,
    lifecycle::{Operation, completion_result, workflow_result},
    service_supervisor::{self, OperationServices},
};
use weasel_common::{
    logging::ComponentLogger,
    process::SingleInstance,
    rpc::{default_pipe_name, default_renderer_pipe_name},
};

/// 是否已有部署或重启操作正在进行或等待 UI 读取结果。
static BUSY: AtomicBool = AtomicBool::new(false);
/// 后台操作尚未被托盘线程读取的结果。
static RESULT: Mutex<Option<OperationOutcome>> = Mutex::new(None);
/// 当前后台工作线程；broker 退出时必须等待其归还服务句柄。
static WORKER: Mutex<Option<thread::JoinHandle<()>>> = Mutex::new(None);

/// 可由任意 UI 展示的后台操作结果。
pub(crate) struct OperationOutcome {
    /// 空字符串表示操作成功且无需提示。
    pub(crate) message: String,
    /// 是否应按错误结果呈现。
    pub(crate) failed: bool,
}

impl OperationOutcome {
    fn success() -> Self {
        Self {
            message: String::new(),
            failed: false,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            failed: true,
        }
    }
}

/// 返回当前是否有操作占用服务所有权或等待结果显示。
pub(crate) fn is_busy() -> bool {
    BUSY.load(Ordering::Acquire)
}

/// 返回是否有尚未被 UI 读取的操作结果。
pub(crate) fn has_result() -> bool {
    RESULT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_some()
}

/// 取走操作结果并释放忙碌状态。
pub(crate) fn take_result() -> Option<OperationOutcome> {
    let result = RESULT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if result.is_some() {
        BUSY.store(false, Ordering::Release);
    }
    result
}

/// 启动串行的部署或重启工作线程。
///
/// `notify` 仅表示“结果可读取”，可安全地把通知转发到任意 UI 消息循环。并发请求会被忽略。
pub(crate) fn begin(operation: Operation, notify: impl Fn() + Send + Sync + 'static) {
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    if let Some(previous) = WORKER.lock().unwrap().take()
        && previous.join().is_err()
    {
        service_supervisor::diagnostic("Previous broker operation worker panicked");
    }
    let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);
    let worker_notify = Arc::clone(&notify);
    let started = thread::Builder::new()
        .name("weasel-operation".to_owned())
        .spawn(move || {
            let Some(mut services) = service_supervisor::acquire_for_operation(operation) else {
                *RESULT.lock().unwrap_or_else(|error| error.into_inner()) = Some(
                    OperationOutcome::error("Broker service state is unavailable"),
                );
                worker_notify();
                return;
            };
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match operation {
                    Operation::Deploy => deploy(&mut services),
                    Operation::Restart => restart_components(&mut services),
                    _ => OperationOutcome::error("Unsupported broker operation"),
                }))
                .unwrap_or_else(|_| {
                    OperationOutcome::error("Broker operation panicked; child ownership restored")
                });
            if result.failed {
                service_supervisor::diagnostic(&result.message);
            }
            let failed = result.failed;
            service_supervisor::restore_after_operation(operation, services, failed);
            *RESULT.lock().unwrap_or_else(|error| error.into_inner()) = Some(result);
            worker_notify();
        });
    match started {
        Ok(worker) => *WORKER.lock().unwrap() = Some(worker),
        Err(error) => {
            *RESULT.lock().unwrap_or_else(|error| error.into_inner()) = Some(
                OperationOutcome::error(format!("无法启动后台操作：{error}")),
            );
            notify();
        }
    }
}

/// 等待当前后台操作结束；用于 broker 退出收尾。
pub(crate) fn join() {
    if let Some(worker) = WORKER
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
        && worker.join().is_err()
    {
        service_supervisor::diagnostic("Operation worker panicked during shutdown");
    }
}

/// 在截止时间内观察异步操作，并定期检查 broker 是否正在退出。
async fn bounded_operation<T>(
    future: impl std::future::Future<Output = Result<T, String>>,
    limit: Duration,
) -> Result<T, String> {
    let deadline = Instant::now() + limit;
    let mut future = std::pin::pin!(future);
    loop {
        if service_supervisor::is_stopping() {
            return Err("broker 关闭打断了部署观察，部署进程仍在运行".into());
        }
        if Instant::now() >= deadline {
            return Err("等待部署超时，部署进程仍在运行".into());
        }
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(100), future.as_mut()).await
        {
            return result;
        }
    }
}

/// 部署提示：在独立线程里弹一个 TaskDialog，避免阻塞 broker 的工作流程。
///
/// 部署可能持续数分钟（字典全量重建），用户需要知道"正在进行 / 已完成 / 无需部署"，
/// 否则会以为程序卡住了。
fn notify_deploy(main: &str, content: String) {
    let main = main.to_string();
    std::thread::spawn(move || {
        let _ = weasel_common::task_dialog::TaskDialog::new("小狼毫Next", main)
            .icon(weasel_common::task_dialog::TaskDialogIcon::Information)
            .content(content)
            .buttons(weasel_common::task_dialog::TaskDialogButtons::Ok)
            .show();
    });
}

/// 执行部署 UI 工作流，并在部署方释放数据后恢复普通 server。
fn deploy(services: &mut OperationServices) -> OperationOutcome {
    let result = (|| -> Result<(), String> {

        // 源文件内容哈希未变时整段跳过部署：不关 server、零停机、零耗时。
        // librime 只比对 mtime；源文件被 touch（内容一个字节没变）也会触发分钟级全量重建。
        {
            use std::io::Write as _;
            let manifest_path =
                weasel_common::deploy_manifest::manifest_path(&services.paths.user_data);
            if let Ok(current) =
                weasel_common::deploy_manifest::collect(&services.paths.user_data)
            {
                let stored = weasel_common::deploy_manifest::load(&manifest_path);
                if weasel_common::deploy_manifest::is_unchanged(stored.as_ref(), &current) {
                    let mut log = ComponentLogger::for_paths(&services.paths, "broker-deploy")
                        .map_err(|error| error.to_string())?;
                    let _ = writeln!(
                        log,
                        "[broker] 源文件哈希未变（{} 项），无需部署，已跳过",
                        current.len()
                    );
                    let _ = log.flush();
                    notify_deploy(
                        "无需部署",
                        format!(
                            "源文件内容未变化（{} 项哈希一致），已跳过部署 ✓",
                            current.len()
                        ),
                    );
                    return Ok(());
                }
            }
        }
        service_supervisor::shutdown_server(services)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime.block_on(async {
            use std::io::Write;
            use weasel_common::deploy_protocol::wait_for_completion;

            let mut log = ComponentLogger::for_paths(&services.paths, "broker-deploy")
                .map_err(|error| error.to_string())?;
            let mut child = tokio::process::Command::new(
                services
                    .paths
                    .executable_directory
                    .join("Weasel-Next-Engine.exe"),
            )
            .arg("--deploy-ui")
            .current_dir(&services.paths.executable_directory)
            .creation_flags(CREATE_NO_WINDOW as u32)
            .kill_on_drop(false)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("无法启动部署界面：{error}"))?;
            let _ = writeln!(log, "[broker] 部署界面已启动，进程号 {:?}", child.id());
            let mut stdout = child.stdout.take().unwrap();
            let result = bounded_operation(
                async {
                    let done = wait_for_completion(&mut stdout, |_| Ok(()))
                        .await
                        .map_err(|error| error.to_string())?;
                    let _ = writeln!(
                        log,
                        "\n[broker] received Complete success={} exit_code={:?}; detaching UI",
                        done.success, done.exit_code
                    );
                    log.write_all(format!("\n{}\n", done.message).as_bytes())
                        .map_err(|error| error.to_string())?;
                    completion_result(done.success, done.exit_code, &done.message)
                },
                Duration::from_secs(300),
            )
            .await;
            let _ = log.flush();
            // 完成协议到达后部署 UI 仍可显示结果窗口，不需要等待它退出。
            drop(child);
            result
        })?;
        Ok(())
    })();

    // 部署成功后记录源文件清单：下次内容未变即可整段跳过。
    if result.is_ok() {
        if let Ok(current) = weasel_common::deploy_manifest::collect(&services.paths.user_data) {
            let path = weasel_common::deploy_manifest::manifest_path(&services.paths.user_data);
            let _ = weasel_common::deploy_manifest::store(&path, &current);
        }
    }

    // 用户可见的结果提示：完成 / 失败（失败带原因，便于自助排查）。
    match &result {
        Ok(()) => notify_deploy("部署完成", "方案部署完成 ✓".to_string()),
        Err(error) => notify_deploy("部署失败", error.clone()),
    }

    let mut restoration = Ok(());
    if services.server.is_none() {
        // 部署 UI 崩溃后，其工作线程仍可能短暂持有 Rime 数据锁。
        if !service_supervisor::is_stopping() {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match SingleInstance::acquire("server") {
                    Ok(guard) => {
                        drop(guard);
                        break;
                    }
                    Err(weasel_common::process::SingleInstanceError::AlreadyRunning) => {
                        if service_supervisor::is_stopping() || Instant::now() >= deadline {
                            let error = workflow_result(
                                result,
                                Err(
                                    "server instance lock remained busy; automatic retry deferred"
                                        .into(),
                                ),
                            )
                            .unwrap_err();
                            service_supervisor::diagnostic(&error);
                            return OperationOutcome::error(error);
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                    Err(lock_error) => {
                        let error = workflow_result(
                            result,
                            Err(format!("无法确认部署进程已释放数据：{lock_error}")),
                        )
                        .unwrap_err();
                        service_supervisor::diagnostic(&error);
                        return OperationOutcome::error(error);
                    }
                }
            }
        }
        if service_supervisor::is_stopping() {
            let error =
                workflow_result(result, Err("restoration skipped: broker is exiting".into()))
                    .unwrap_err();
            service_supervisor::diagnostic(&error);
            return OperationOutcome::error(error);
        }
        match service_supervisor::start_child(
            &services.paths.executable_directory,
            "Weasel-Next-Engine.exe",
            &[],
        ) {
            Ok(mut child) => {
                service_supervisor::diagnostic(&format!(
                    "ordinary server spawned pid={}",
                    child.id()
                ));
                match service_supervisor::wait_for_server(&mut child) {
                    Ok(()) => {
                        service_supervisor::diagnostic("ordinary server answered readiness ping")
                    }
                    Err(error) => {
                        service_supervisor::diagnostic(&error);
                        restoration = Err(error);
                    }
                }
                services.server = Some(child);
            }
            Err(error) => restoration = Err(format!("server restart failed: {error}")),
        }
    }
    match workflow_result(result, restoration) {
        Err(error) => {
            service_supervisor::diagnostic(&format!("deployment workflow failed: {error}"));
            OperationOutcome::error(format!("部署流程异常：\n{error}"))
        }
        Ok(()) => OperationOutcome::success(),
    }
}

/// 停止并重新启动两项服务，同时刷新设置和通知状态。
fn restart_components(services: &mut OperationServices) -> OperationOutcome {
    let mut errors = Vec::new();
    if let Err(error) = service_supervisor::shutdown_component(
        &mut services.server,
        default_pipe_name(),
        "broker requested restart",
    ) {
        return OperationOutcome::error(format!("无法停止 server，已取消重启：{error}"));
    }
    if let Err(error) = service_supervisor::shutdown_component(
        &mut services.renderer,
        default_renderer_pipe_name(),
        "broker requested restart",
    ) {
        return OperationOutcome::error(format!("无法停止 renderer，已取消重启：{error}"));
    }

    // renderer 启动时会立即查询主题，因此必须先同步发布新设置。
    services.notifications.reset();
    let mut settings_warnings = Vec::new();
    let settings = crate::settings::load(&services.paths, |warning| {
        service_supervisor::diagnostic(&warning);
        settings_warnings.push(warning);
    });
    services.settings.replace(settings);
    services
        .notifications
        .report_settings_errors(&settings_warnings);

    if services.renderer.is_none() && !service_supervisor::is_stopping() {
        match service_supervisor::start_child(
            &services.paths.executable_directory,
            "Weasel-Next-Renderer.exe",
            &[],
        ) {
            Ok(child) => services.renderer = Some(child),
            Err(error) => errors.push(format!("无法启动 renderer：{error}")),
        }
    }
    if errors.is_empty() && services.server.is_none() && !service_supervisor::is_stopping() {
        match service_supervisor::start_child(
            &services.paths.executable_directory,
            "Weasel-Next-Engine.exe",
            &[],
        ) {
            Ok(mut child) => {
                if let Err(error) = service_supervisor::wait_for_server(&mut child) {
                    errors.push(error);
                }
                services.server = Some(child);
            }
            Err(error) => errors.push(format!("无法启动 server：{error}")),
        }
    }
    if errors.is_empty() {
        return OperationOutcome::success();
    }

    // 重启拥有两个句柄；部分恢复失败时关闭已经启动的另一方，避免留下半初始化体系。
    for (child, pipe) in [
        (&mut services.server, default_pipe_name()),
        (&mut services.renderer, default_renderer_pipe_name()),
    ] {
        if let Err(error) = service_supervisor::shutdown_component(child, pipe, "restart rollback")
        {
            errors.push(error);
        }
    }
    OperationOutcome::error(format!("重启流程异常：\n{}", errors.join("\n")))
}

#[cfg(test)]
#[path = "../tests/unit/operations.rs"]
mod tests;
