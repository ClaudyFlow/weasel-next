//! 为进程内 TIP 提供专用的命名管道 RPC 运行时。
//!
//! TSF 在宿主应用的 COM 线程上调用 TIP；此模块把 Tokio 客户端和异步任务
//! 限定在独立工作线程中，避免在 TSF 回调里运行执行器或等待网络响应。
//! 宿主线程通过有界命令队列提交工作，并在有限时限内等待按连接 epoch
//! 校验的按键响应；超时或断连时丢弃迟到结果，使输入可以安全回退。

use std::{
    os::windows::io::AsRawHandle,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::rpc_diagnostics::report;
use weasel_common::message::{
    ContextAction, ContextCommand, InputKey, KeyEventResponse, LayoutUpdate, LogEvent, OpenInput,
    PeerRole, Ping, envelope::Payload,
};
use weasel_common::rpc::{
    RpcClient, RpcError, try_default_broker_pipe_name, try_default_pipe_name,
};

/// 全局连接 epoch 分配器；0 保留为无效标记，分配回绕到 0 时拒绝该连接。
static NEXT_CONNECTION_EPOCH: AtomicU64 = AtomicU64::new(1);
/// 单条 RPC 命令（包括连接握手）的最大处理预算。
const COMMAND_TIMEOUT: Duration = Duration::from_millis(80);
/// 冷启动 / 刚切到某程序时的**首个按键**要顺带完成"连接 + 握手 + open_input"，
/// 80ms 的常规预算几乎必然超时 → 该键按英文回退直接交给应用，用户看到的就是
/// "前几个字没进入输入法"。首键给更宽的预算：它已被输入法吞下，不会漏给应用，
/// 只是可能晚几十毫秒出现在输入框里。
const CONNECT_KEY_TIMEOUT: Duration = Duration::from_millis(500);
/// 后端**可能正在忙**时的按键预算（引擎刚重启、刚切上下文/页面、刚切中英）。
///
/// 这些时刻 RPC 连接还在，走不到上面的 `CONNECT_KEY_TIMEOUT` 分支；但后端要重建会话
/// 或等一次布局往返，80ms 常常不够 → 前几个字母被英文直通给应用。按键已经被输入法
/// 吞下，多等一会儿只是晚几十毫秒出现，不会漏给应用，所以这里统一放宽。
const BUSY_KEY_TIMEOUT: Duration = Duration::from_millis(600);
/// 一条命令**轮到被派发时**至少还要有的预算。
///
/// 用得着它的场景是"排队被拖过期"：首键在跑连接握手（最长 500ms），后面几个键
/// 的 deadline 是入队时按 80ms 定的，轮到它们时早已过期。那些键已经被输入法吞下，
/// 直接丢会让开头几个字母凭空消失；给一个下限让它们仍然被送出去。
const MIN_DISPATCH_BUDGET: Duration = Duration::from_millis(120);
/// 宿主线程等待按键结果时比预算多留的余量，避免在边界上误判超时。
const HOST_WAIT_MARGIN: Duration = Duration::from_millis(60);
/// 停止工作线程时等待原生线程退出的最长毫秒数。
const STOP_TIMEOUT_MS: u32 = 100;

#[derive(Debug, PartialEq, Eq)]
/// 向工作线程提交命令失败的原因。
pub(crate) enum CommandError {
    /// RPC 工作线程尚未启动。
    NotStarted,
    /// 工作线程已退出，命令接收端已关闭。
    Closed,
    /// 有界队列已满；调用方应按当前操作的策略处理背压。
    Full,
}

/// 私有 Tokio 运行时串行处理的工作项。
enum RpcCommand {
    /// 尽力发送诊断日志，不等待服务器确认。
    Log { level: String, text: String },
    /// 处理需在宿主线程等待的按键，并携带不可续期的截止时间。
    KeyEvent {
        event: InputKey,
        response: Sender<Option<KeyEventResponse>>,
        deadline: Instant,
    },
    /// 发布最新布局状态；此类状态通过合并邮箱而非命令队列传递。
    LayoutUpdate(LayoutUpdate),
    /// 处理输入上下文的焦点、失焦或销毁等命令。
    Context(ContextCommand),
}

#[derive(Default)]
/// 工作线程与宿主 UI 线程共享的推送状态及连接代际门闩。
///
/// 原子字段允许跨线程读取轻量状态；响应队列由互斥锁保护，并且在 UI 边界
/// 使用非阻塞清理，避免 COM 回调因后台持锁而等待。
struct PushTarget {
    // 0 = unknown, 1 = Chinese, 2 = English. Only confirmed UI-thread replies
    // update this memory; connection invalidation deliberately preserves it.
    /// 最近一次由 UI 线程确认的 ASCII 模式：0 未知、1 中文、2 英文。
    remembered_ascii: AtomicUsize,
    /// 仅保留最新布局的 watch 邮箱，避免布局洪泛挤占交互命令。
    layout: LayoutMailbox,
    /// 接收更新通知的宿主窗口句柄；0 表示当前没有可通知窗口。
    window: AtomicUsize,
    /// 等待 UI 线程按上下文和连接 epoch 取走的服务器主动响应。
    updates: Mutex<Vec<KeyEventResponse>>,
    /// 主动推送通道发生不可恢复故障时置位，按键处理据此停止。
    failed: AtomicBool,
    /// 后端可能正在忙（连接失效、刚切上下文/页面/模式、上一次按键超时）。
    ///
    /// 置位时按键改用 [`BUSY_KEY_TIMEOUT`]；不设这个标志就会出现"引擎刚重启或刚切
    /// 上下文时前几个字母被英文直通"的老问题。任意一次按键成功应答即清除。
    slow: AtomicBool,
    /// 当前 RPC 连接不可用；可在下一次需要后端时重新连接。
    connection_failed: AtomicBool,
    /// 已完成握手的连接编号；0 表示当前没有有效连接。
    epoch: AtomicU64,
    /// 每次失效递增，使握手期间被取消的连接不能重新发布为可用。
    cancellation: AtomicU64,
    /// 建立当前 epoch 时捕获的取消代际，读取时必须与当前代际一致。
    connection_generation: AtomicU64,
}

/// 只保存最后一条布局更新的合并邮箱。
struct LayoutMailbox(tokio::sync::watch::Sender<Option<LayoutUpdate>>);
impl Default for LayoutMailbox {
    /// 创建当前值为空的 watch 邮箱。
    fn default() -> Self {
        Self(tokio::sync::watch::channel(None).0)
    }
}

impl PushTarget {
    /// 返回最近一次由宿主 UI 确认的模式；未知时返回 `None`。
    fn remembered_mode(&self) -> Option<bool> {
        match self.remembered_ascii.load(Ordering::Acquire) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        }
    }
    /// 获取当前仍属于活动连接代际的 epoch，否则返回 0。
    fn current_epoch(&self) -> u64 {
        let generation = self.cancellation.load(Ordering::Acquire);
        let epoch = self.epoch.load(Ordering::Acquire);
        if self.connection_generation.load(Ordering::Acquire) != generation
            || self.cancellation.load(Ordering::Acquire) != generation
        {
            0
        } else {
            epoch
        }
    }

    /// 使连接及其待处理响应立即失效，并唤醒宿主窗口清理状态。
    ///
    /// 该操作不覆盖已确认的输入模式；清理队列时不等待可能被 UI 回调持有的锁。
    fn invalidate(&self) {
        self.cancellation.fetch_add(1, Ordering::AcqRel);
        self.epoch.store(0, Ordering::Release);
        self.connection_failed.store(true, Ordering::Release);
        // 连接刚刚作废（超时/断开）→ 后端接下来多半还要重建会话，按键放宽预算
        self.slow.store(true, Ordering::Release);
        if let Some(mut updates) = crate::boundary::try_teardown(&self.updates) {
            updates.clear();
        }
        self.notify();
    }

    /// 通过 Win32 投递异步更新消息；绝不在 RPC 线程直接调用窗口过程。
    fn notify(&self) {
        let hwnd = self.window.load(Ordering::Acquire);
        if hwnd != 0 {
            unsafe {
                let _ = crate::bindings::PostMessageW(
                    Some(crate::bindings::HWND(hwnd as *mut _)),
                    crate::update_window::UPDATE_MESSAGE,
                    crate::bindings::WPARAM(0),
                    crate::bindings::LPARAM(0),
                );
            }
        }
    }
}

/// 将服务器主动响应放入宿主线程队列，并通知窗口取走。
///
/// 过期 epoch 的响应视为已处理并静默丢弃；有效队列最多容纳 64 项。队列满、
/// 锁中毒或窗口消息投递失败时返回 `false`，由订阅任务隔离故障并断开连接。
fn enqueue_update(target: &PushTarget, epoch: u64, response: KeyEventResponse) -> bool {
    // A late response may still be broadcast after PendingCall was cancelled.
    // Reject it here and again when draining: invalidation can race this check.
    if epoch == 0 || target.current_epoch() != epoch {
        return true;
    }
    let queued = match target.updates.lock() {
        Ok(mut queue) if queue.len() < 64 => {
            queue.push(response);
            true
        }
        _ => false,
    };
    if !queued {
        return false;
    }
    let hwnd = target.window.load(Ordering::Acquire);
    hwnd == 0
        || unsafe {
            crate::bindings::PostMessageW(
                Some(crate::bindings::HWND(hwnd as *mut _)),
                crate::update_window::UPDATE_MESSAGE,
                crate::bindings::WPARAM(0),
                crate::bindings::LPARAM(0),
            )
            .as_bool()
        }
}

/// 一条经过握手的 RPC 会话及其主动响应订阅。
struct Connection {
    /// 请求/发布客户端；其异步任务仅由专用运行时驱动。
    client: RpcClient,
    /// 已成功打开的上下文 ID，避免重复初始化同一输入会话。
    opened: tokio::sync::Mutex<std::collections::HashSet<u64>>,
    /// 转发服务器主动响应的任务；关闭或释放会话时必须中止。
    updates: tokio::task::JoinHandle<()>,
}

impl Connection {
    /// 停止响应订阅并断开传输；在工作线程退出前完成异步清理。
    async fn close(mut self) {
        self.updates.abort();
        let _ = (&mut self.updates).await;
        self.client.disconnect().await;
        // Drop also aborts on cancellation of close(). Runtime destruction is
        // the final fallback, and completes before the worker is joined.
    }

    /// 发布无需请求/响应往返的消息。
    fn publish(&self, payload: Payload) -> Result<(), RpcError> {
        self.client.publish(payload)
    }

    /// 发出请求，并要求服务器以按键响应负载作答。
    ///
    /// 其他负载类型均视为协议错误，由上层统一记录并使会话失效。
    async fn key_request(&self, payload: Payload) -> Result<KeyEventResponse, RpcError> {
        match self.client.request(payload).await?.payload {
            Some(Payload::KeyEventResponse(response)) => Ok(response),
            _ => Err(RpcError::UnexpectedResponse),
        }
    }

    /// 确保上下文已由服务器打开；必要时恢复宿主最近确认的输入模式。
    ///
    /// `opened` 锁跨越 RPC 往返，用于串行化首次打开，避免同一上下文并发初始化。
    /// 只有服务器回显匹配 token 且模式恢复得到确认后，才记录为已打开。
    async fn prepare_input_session(
        &self,
        token: Option<weasel_common::message::ContextToken>,
        initial_ascii_mode: Option<bool>,
    ) -> Result<(), RpcError> {
        let id = token
            .as_ref()
            .ok_or_else(|| RpcError::Protocol("input token required".into()))?
            .context_id;
        let mut opened = self.opened.lock().await;
        if opened.contains(&id) {
            return Ok(());
        }
        let reply = self
            .client
            .request(Payload::OpenInput(OpenInput {
                token: token.clone(),
            }))
            .await?;
        match reply.payload {
            Some(Payload::InputOpened(value)) if value.token == token => {
                if let Some(ascii_mode) = initial_ascii_mode {
                    let response = self
                        .key_request(Payload::ContextCommand(ContextCommand {
                            token: token.clone(),
                            action: ContextAction::SetAscii as i32,
                            ascii_mode: Some(ascii_mode),
                        }))
                        .await?;
                    if response.token != token || response.ascii_mode != Some(ascii_mode) {
                        return Err(RpcError::Protocol(
                            "input mode restoration was not confirmed".into(),
                        ));
                    }
                }
                opened.insert(id);
                Ok(())
            }
            _ => Err(RpcError::UnexpectedResponse),
        }
    }

    /// 在已打开的输入会话中处理翻译后的按键。
    async fn process_translated_key(&self, event: InputKey) -> Result<KeyEventResponse, RpcError> {
        // 输入统计：每个键都过这里，是最可靠的挂点。
        // 传 `virtual_key`（Win32 虚拟键）而不是 Rime 的 `keycode`：
        // 键盘热力图要按真实键位统计，0xFFE3 这类扩展键码只有它能区分。
        crate::stats::record_key(event.virtual_key as i64);
        self.prepare_input_session(event.token.clone(), None)
            .await?;
        let response = self.key_request(Payload::KeyEvent(event)).await?;
        // 上屏字数按**真正提交给宿主的文本**算（不是候选长度）。
        crate::stats::record_commit(&response.commit_text);
        Ok(response)
    }

    /// 转发上下文命令；未打开上下文的销毁操作作为空操作处理。
    ///
    /// 只有服务器成功处理销毁后才从本地已打开集合移除该上下文。
    async fn context_command(&self, command: ContextCommand) -> Result<KeyEventResponse, RpcError> {
        let destroy = command.action == ContextAction::Destroy as i32;
        let id = command
            .token
            .as_ref()
            .map(|token| token.context_id)
            .unwrap_or(0);
        if destroy && !self.opened.lock().await.contains(&id) {
            return Ok(KeyEventResponse::default());
        }
        self.prepare_input_session(command.token.clone(), None)
            .await?;
        let response = self.key_request(Payload::ContextCommand(command)).await;
        if destroy && response.is_ok() {
            self.opened.lock().await.remove(&id);
        }
        response
    }
}

impl std::ops::Deref for Connection {
    type Target = RpcClient;
    fn deref(&self) -> &RpcClient {
        &self.client
    }
}

impl Drop for Connection {
    /// 确保即使异步关闭被取消，订阅任务也不会因客户端克隆而滞留。
    fn drop(&mut self) {
        // The subscriber owns a client clone, whose broadcast senders keep its
        // own receiver alive. Never rely on RecvError::Closed to end this task.
        self.updates.abort();
    }
}

/// 连接并完成 Ping/Pong 握手，建立带独立 epoch 的主动响应订阅。
///
/// 在握手前捕获取消代际；若期间会话已失效则拒绝发布连接可用状态。连接事件
/// 丢失、订阅溢出或无法投递响应时，订阅任务会使 epoch 失效并断开传输。
/// 引擎管道连不上时向 broker 管道发一次 Ping,把空闲自退的后端拉起来。
/// 尽力而为:broker 不在(整体未启动)时静默放弃,重试循环会再试。
async fn wake_broker() {
    let Ok(name) = try_default_broker_pipe_name() else {
        return;
    };
    let client = match RpcClient::connect_as(&name, PeerRole::Tip).await {
        Ok(client) => client,
        Err(_) => return,
    };
    let _ = client
        .request(Payload::Ping(Ping {
            text: "tip idle wake".into(),
        }))
        .await;
    client.disconnect().await;
}

async fn connect_client(pipe_name: &str, target: &Arc<PushTarget>) -> Option<Connection> {
    let generation = target.cancellation.load(Ordering::Acquire);
    let client = RpcClient::connect_as(pipe_name, PeerRole::Tip)
        .await
        .inspect_err(|error| report("pipe-connect", Some(pipe_name), error))
        .ok()?;
    let mut updates = client.subscribe();

    // Treat the handshake as part of connection establishment.  A pipe can
    // be opened just before the server exits, so connect() alone is not
    // enough to consider the server usable.
    match client
        .request(Payload::Ping(Ping {
            text: "tip activated".into(),
        }))
        .await
        .and_then(|envelope| match envelope.payload {
            Some(Payload::Pong(_)) => Ok(()),
            _ => Err(RpcError::UnexpectedResponse),
        }) {
        Ok(()) => {}
        Err(error) => {
            report("handshake-ping", Some(pipe_name), error);
            return None;
        }
    }
    if target.cancellation.load(Ordering::Acquire) != generation {
        report(
            "handshake-cancelled",
            Some(pipe_name),
            "connection was invalidated during handshake",
        );
        client.disconnect().await;
        return None;
    }
    let epoch = NEXT_CONNECTION_EPOCH.fetch_add(1, Ordering::AcqRel);
    if epoch == 0 {
        report("connection-epoch", Some(pipe_name), "epoch exhausted");
        client.disconnect().await;
        return None;
    }
    target.connection_failed.store(false, Ordering::Release);
    target
        .connection_generation
        .store(generation, Ordering::Release);
    target.epoch.store(epoch, Ordering::Release);
    let target = target.clone();
    let push_client = client.clone();
    let updates = tokio::spawn(async move {
        loop {
            let envelope = match tokio::select! {
                _ = push_client.disconnected() => {
                    if target.current_epoch() == epoch { target.invalidate(); }
                    break;
                }
                response = updates.recv() => response,
            } {
                Ok(response) => response,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    report("server-events", None, "reply subscription overflowed");
                    if target.current_epoch() == epoch {
                        target.failed.store(true, Ordering::Release);
                        target.invalidate();
                    }
                    push_client.disconnect().await;
                    break;
                }
            };
            let Some(Payload::KeyEventResponse(response)) = envelope.payload else {
                continue;
            };
            if !enqueue_update(&target, epoch, response) {
                report(
                    "server-events",
                    None,
                    "could not enqueue reply for the host thread",
                );
                if target.current_epoch() == epoch {
                    target.failed.store(true, Ordering::Release);
                    target.invalidate();
                }
                push_client.disconnect().await;
                break;
            }
        }
    });
    Some(Connection {
        client,
        opened: tokio::sync::Mutex::new(std::collections::HashSet::new()),
        updates,
    })
}

impl RpcCommand {
    /// 返回命令的绝对截止时间；按键沿用入队时的时限，其他命令使用固定预算。
    fn deadline(&self) -> Instant {
        match self {
            Self::KeyEvent { deadline, .. } => *deadline,
            _ => Instant::now() + COMMAND_TIMEOUT,
        }
    }
}

/// 由私有运行时线程独占的连接与命令状态。
///
/// 所有客户端创建、请求和关闭都在该线程内串行执行，宿主 COM 线程只接触
/// `PushTarget` 和命令通道。
struct WorkerState {
    /// 连接服务器命名管道所用的名称。
    pipe_name: String,
    /// 当前经过握手的会话；空值表示尚未连接或已被丢弃。
    client: Option<Connection>,
    /// 与宿主线程共享的通知队列及 epoch 状态。
    push: Arc<PushTarget>,
}

impl WorkerState {
    /// 串行消费命令和布局变更，直到停止信号或所有命令发送端关闭。
    ///
    /// 每项工作受其原始截止时间限制；超时后先使连接和竞态中的迟到响应失效，
    /// 再继续循环。退出时等待会话清理完成。
    async fn run(
        mut self,
        mut commands: tokio::sync::mpsc::Receiver<RpcCommand>,
        mut stop: tokio::sync::oneshot::Receiver<()>,
    ) {
        let mut layouts = self.push.layout.0.subscribe();
        loop {
            let command = tokio::select! {
                biased;
                _ = &mut stop => break,
                command = commands.recv() => command,
                changed = layouts.changed() => {
                    if changed.is_err() { break; }
                    let latest = layouts.borrow_and_update().clone();
                    let Some(update) = latest else { continue };
                    Some(RpcCommand::LayoutUpdate(update))
                }
            };
            let Some(command) = command else { break };
            // ⚠️ 排队期间被"拖过期"的按键不能直接丢！
            //
            // 场景（用户报"打快了前几个字母不识别"）：冷启动/刚切程序时首键要走
            // "连接 + 握手 + open_input"，它自己拿到 CONNECT_KEY_TIMEOUT(500ms)，但
            // **它后面排队的第 2、3 个键，deadline 是入队那一刻就按常规预算(80ms)定死的**。
            // 首键占满 500ms 之后，队列里那几个键的 80ms 早已过期 → 在这里被 `continue`
            // 丢掉 ⇒ 开头几个字母既不进输入法也不进应用（凭空消失）。
            //
            // 这些键**已经被输入法吞下**（不会漏给应用），多等几十毫秒只是晚一点出现，
            // 所以轮到它们时至少给 MIN_DISPATCH_BUDGET 的预算，而不是按入队时间判死。
            let arrival = Instant::now();
            let deadline = command.deadline().max(arrival + MIN_DISPATCH_BUDGET);
            self.discard_disconnected_client().await;

            // 建立连接时的首键：放宽预算，避免这一键被英文回退吞掉；
            // 连接就绪后仍按常规预算，Await 串行以保证按键回复与上下文命令的顺序。
            let establishing = self.client.is_none();
            // 注意：入队时可能还没置位 slow（例如刚切上下文那一瞬），所以这里再兜一层
            let busy = self.push.slow.load(Ordering::Acquire);
            let dispatch_deadline = if establishing || busy {
                Instant::now() + CONNECT_KEY_TIMEOUT.max(BUSY_KEY_TIMEOUT)
            } else {
                deadline
            };
            let result = tokio::select! {
                biased;
                _ = &mut stop => break,
                result = tokio::time::timeout_at(dispatch_deadline.into(), self.dispatch(command)) => result,
            };
            if result.is_err() {
                report(
                    if establishing {
                        "connect-or-handshake-timeout"
                    } else {
                        "request-timeout"
                    },
                    Some(&self.pipe_name),
                    "command deadline exceeded (80ms budget)",
                );
                // Invalidate queued racing replies before English fallback.
                self.disconnect_invalidated_client().await;
            }
        }
        if let Some(current) = self.client.take() {
            current.close().await;
        }
    }

    /// 按需连接；连接或握手失败时标记状态并通知宿主线程。
    ///
    /// 连接失败还有一个重要副作用:**唤醒 broker**。引擎按"五分钟不打字
    /// 自退"设计会主动退出(Rime-Next 2026-10-27),broker 收到本 Tip 的
    /// 管道活动后就地拉起引擎与渲染器——下一次 ensure_connected 就能连上。
    /// 本键穿透(字母直接上屏)是空闲唤醒的预期代价。
    async fn ensure_connected(&mut self) {
        if self.client.is_none() {
            self.client = connect_client(&self.pipe_name, &self.push).await;
            if self.client.is_none() {
                self.push.connection_failed.store(true, Ordering::Release);
                self.push.notify();
                wake_broker().await;
            }
        }
    }

    /// 先使共享 epoch 失效，再取出当前会话供调用方关闭。
    fn invalidate_connection(&mut self) -> Option<Connection> {
        self.push.invalidate();
        self.client.take()
    }

    /// 使连接失效并完成异步断开；不得在宿主 COM 回调中调用。
    async fn disconnect_invalidated_client(&mut self) {
        if let Some(current) = self.invalidate_connection() {
            current.close().await;
        }
    }

    /// 在分发下一条命令前清除传输已断开或 epoch 已取消的会话。
    async fn discard_disconnected_client(&mut self) {
        if self
            .client
            .as_ref()
            .is_some_and(|current| !current.is_connected() || self.push.current_epoch() == 0)
        {
            report(
                "connection-invalidated",
                Some(&self.pipe_name),
                "transport closed or epoch cancelled",
            );
            self.disconnect_invalidated_client().await;
        }
    }

    /// 分派单条命令；按键结果在截止时间已过或接收方消失时不再交付。
    async fn dispatch(&mut self, command: RpcCommand) {
        match command {
            RpcCommand::Log { level, text } => self.handle_log(level, text).await,
            RpcCommand::KeyEvent {
                event,
                response,
                deadline,
            } => {
                // 已经过了宿主给的时限：这一键宿主早就放弃等待了。
                // 这里**不连接、不处理、也不拆连接** —— 处理了结果也没人要，
                // 白白踢掉一条健康的连接反而害得下一键必须重连（用户看到的
                // 就是"首键偶尔丢字"）。直接丢掉，保持现状。
                if Instant::now() >= deadline {
                    drop(response);
                    return;
                }
                let value = self.handle_key_event(event).await;
                if Instant::now() >= deadline || response.send(value).is_err() {
                    self.disconnect_invalidated_client().await;
                }
            }
            RpcCommand::Context(command) => self.handle_context(command).await,
            RpcCommand::LayoutUpdate(update) => self.handle_layout(update).await,
        }
    }

    /// 尽力发送日志；失败仅记录诊断并隔离失效连接。
    async fn handle_log(&mut self, level: String, text: String) {
        self.ensure_connected().await;
        let Some(current) = self.client.as_ref() else {
            return;
        };
        if let Err(error) = current.publish(Payload::LogEvent(LogEvent { level, text })) {
            report("send-diagnostic", Some(&self.pipe_name), error);
            self.disconnect_invalidated_client().await;
        }
    }

    /// 连接并处理按键，校验响应 token 与连接 epoch 后才将结果交还宿主。
    ///
    /// 任何连接、会话恢复、协议或响应校验错误均被隔离为 `None`，使 TSF 调用方
    /// 可以走英文直通路径；不会把不匹配或已取消会话的响应交给 COM 边界。
    async fn handle_key_event(&mut self, mut event: InputKey) -> Option<KeyEventResponse> {
        self.ensure_connected().await;
        if let Some(token) = event.token.as_mut() {
            token.connection_epoch = self.push.current_epoch();
        }
        let expected = event.token.clone();
        let current = self.client.as_ref()?;
        if let Err(error) = current
            .prepare_input_session(event.token, self.push.remembered_mode())
            .await
        {
            report("restore-input-mode", Some(&self.pipe_name), error);
            self.disconnect_invalidated_client().await;
            return None;
        }
        match current.process_translated_key(event).await {
            Ok(value) if self.push.current_epoch() != 0 && value.token == expected => Some(value),
            Ok(_) => {
                report(
                    "key-response",
                    Some(&self.pipe_name),
                    "response token mismatch or epoch cancelled",
                );
                self.disconnect_invalidated_client().await;
                None
            }
            Err(error) => {
                report("key-request", Some(&self.pipe_name), error);
                self.disconnect_invalidated_client().await;
                None
            }
        }
    }

    /// 在后台线程处理焦点/上下文命令；仅非失焦、非销毁命令需要建立会话。
    ///
    /// 焦点查询和模式恢复通过 RPC 异步完成，不在宿主 UI/COM 线程同步等待。
    async fn handle_context(&mut self, mut command: ContextCommand) {
        let blur_only = matches!(
            ContextAction::try_from(command.action),
            Ok(ContextAction::Blur | ContextAction::Destroy)
        );
        // Focus establishes the session and queries its actual input mode in
        // this private runtime, never synchronously on the host COM thread.
        if !blur_only {
            self.ensure_connected().await;
        }
        if let Some(token) = command.token.as_mut() {
            token.connection_epoch = self.push.current_epoch();
        }
        let Some(current) = self.client.as_ref() else {
            return;
        };
        if !blur_only {
            if let Err(error) = current
                .prepare_input_session(command.token, self.push.remembered_mode())
                .await
            {
                report("restore-input-mode", Some(&self.pipe_name), error);
                self.disconnect_invalidated_client().await;
                return;
            }
        }
        if let Err(error) = current.context_command(command).await {
            report("context-request", Some(&self.pipe_name), error);
            self.disconnect_invalidated_client().await;
        }
    }

    /// 发布布局变更；丢弃旧 epoch 更新，且不为布局消息单独重连空闲管道。
    async fn handle_layout(&mut self, update: LayoutUpdate) {
        if update
            .token
            .as_ref()
            .is_some_and(|token| token.connection_epoch != self.push.current_epoch())
        {
            return;
        }
        // Layout must never reconnect an idle pipe.
        let Some(current) = self.client.as_ref() else {
            return;
        };
        if let Err(error) = current.publish(Payload::LayoutUpdate(update)) {
            report("layout-send", Some(&self.pipe_name), error);
            self.disconnect_invalidated_client().await;
        }
    }
}

/// 在线程中创建单线程 Tokio 运行时并运行 RPC 状态机。
///
/// 清理边界捕获线程入口的异常；运行时创建失败会记录诊断并使共享状态失效。
fn run_thread(
    state: WorkerState,
    commands: tokio::sync::mpsc::Receiver<RpcCommand>,
    stop: tokio::sync::oneshot::Receiver<()>,
) {
    crate::boundary::cleanup(|| {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                report("runtime-start", Some(&state.pipe_name), error);
                state.push.invalidate();
                return;
            }
        };
        runtime.block_on(state.run(commands, stop));
    });
}

/// TSF 侧 RPC 工作线程的生命周期控制器。
///
/// 可从宿主线程提交有界命令并读取原子状态；网络 I/O 始终由专用线程处理。
/// 按键调用有严格等待上限，停止时等待原生线程退出也有上限。
pub struct RpcWorker {
    /// 通知私有运行时停止的单次信号发送端。
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    /// 私有运行时线程句柄；模块租约必须至少保留到该线程完成退出。
    thread: Option<JoinHandle<()>>,
    /// 有界 RPC 命令通道；使用 `Arc` 供调用方轻量共享发送能力。
    commands: Option<Arc<tokio::sync::mpsc::Sender<RpcCommand>>>,
    /// 当前工作代际共享的响应、窗口和连接状态。
    push: Arc<PushTarget>,
    // Acquired before spawn; released only after join, including thread epilogue
    // and runtime destruction. A lease dropped inside run_thread is too early.
    // On timeout the lease is intentionally retained until process exit.
    module: Option<crate::module::ModuleLease>,
}

impl RpcWorker {
    /// 返回 UI 线程最近确认的输入模式；尚无确认时为 `None`。
    pub fn remembered_mode(&self) -> Option<bool> {
        self.push.remembered_mode()
    }

    /// 保存宿主 UI 已确认的输入模式，供后续新连接恢复。
    pub fn remember_mode(&self, mode: Option<bool>) {
        self.push.remembered_ascii.store(
            match mode {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            },
            Ordering::Release,
        );
    }

    /// 解析默认管道名称并启动工作线程；连接在首次实际请求时惰性建立。
    pub fn start(&mut self) {
        self.start_resolved(try_default_pipe_name());
    }

    /// 启动前停止旧代工作线程；管道身份解析失败时不创建线程并报告故障。
    fn start_resolved(&mut self, pipe: std::io::Result<String>) {
        match pipe {
            Ok(pipe) => self.start_with_pipe(pipe),
            Err(error) => {
                report("pipe-identity", None, error);
                self.stop();
                self.push.connection_failed.store(true, Ordering::Release);
            }
        }
    }

    /// 创建有界命令通道、模块租约及专用线程，不在激活阶段连接服务器。
    fn start_with_pipe(&mut self, pipe_name: String) {
        self.stop();

        let (stop, stop_receiver) = tokio::sync::oneshot::channel();
        let (command_sender, command_receiver) = tokio::sync::mpsc::channel(64);
        // Do not connect during activation; dispatch establishes connections lazily.
        let state = WorkerState {
            pipe_name,
            client: None,
            push: self.push.clone(),
        };
        let module = crate::module::ModuleLease::new();
        let thread = thread::Builder::new()
            .name("weasel-tip-rpc".to_owned())
            .spawn(move || run_thread(state, command_receiver, stop_receiver))
            .inspect_err(|error| {
                report("worker-start", None, error);
                self.push.invalidate();
            })
            .ok();

        self.stop = thread.as_ref().map(|_| stop);
        self.commands = thread.as_ref().map(|_| Arc::new(command_sender));
        self.module = thread.as_ref().map(|_| module);
        self.thread = thread;
    }

    /// 将按键提交到后台，并在宿主线程最多等待 100 毫秒取得结果。
    ///
    /// 入队截止时间为 80 毫秒且不会在出队时续期。超时会在宿主线程同步使 epoch
    /// 失效，防止运行时调度延迟期间到达的响应被误用；失败返回 `None` 供输入直通。
    pub fn process_key_event(&self, event: InputKey) -> Option<KeyEventResponse> {
        if self.push.failed.load(Ordering::Acquire) {
            return None;
        }
        let Some(commands) = self.commands.as_ref() else {
            return None;
        };
        let (response, receiver) = mpsc::channel();
        // 后端忙（刚重启/刚切上下文/上次超时）时给更宽的预算，避免这一键被英文直通
        let busy = self.push.slow.load(Ordering::Acquire);
        // ⚠️ **还没连上时（epoch == 0），宿主也要按"首键"给宽预算。**
        //
        // worker 侧为这一刻留了 CONNECT_KEY_TIMEOUT(500ms) 去完成
        // 「连接 + 握手 + open_input」，但宿主侧原来只按 COMMAND_TIMEOUT(80ms)
        // 加 60ms 余量等 —— **宿主 140ms 就先放弃了，worker 那边还在连**，
        // 于是首键的结果永远取不到、直接 `None` 返回。
        // 用户看到的就是"第一个字缺字"（键被输入法吞下，既没进输入法也没回给应用）。
        // 两边预算必须对齐，否则 worker 给多宽的预算都是白给。
        let establishing = self.connection_epoch() == 0;
        let budget = if establishing {
            CONNECT_KEY_TIMEOUT
        } else if busy {
            BUSY_KEY_TIMEOUT
        } else {
            COMMAND_TIMEOUT
        };
        if commands
            .try_send(RpcCommand::KeyEvent {
                event,
                response,
                deadline: Instant::now() + budget,
            })
            .inspect_err(|error| report("key-queue", None, error))
            .is_err()
        {
            return None;
        }

        match receiver.recv_timeout(budget + HOST_WAIT_MARGIN) {
            Ok(Some(value)) => {
                // 这一次往返是通的 → 后端恢复正常，后续按键回到常规预算
                self.push.slow.store(false, Ordering::Release);
                Some(value)
            }
            result => {
                if let Err(error) = result {
                    report("host-wait", None, error);
                }
                // The runtime can be descheduled past its 80ms timer. The host
                // must invalidate synchronously before passing the key through.
                self.push.invalidate();
                None
            }
        }
    }

    /// 返回当前有效连接的 epoch；未连接、已取消或代际不一致时为 0。
    pub fn connection_epoch(&self) -> u64 {
        self.push.current_epoch()
    }

    /// 指示连接或主动推送是否失败，或当前 epoch 是否已被取消。
    pub fn connection_failed(&self) -> bool {
        self.push.connection_failed.load(Ordering::Acquire)
            || self.push.failed.load(Ordering::Acquire)
            || (self.push.epoch.load(Ordering::Acquire) != 0 && self.push.current_epoch() == 0)
    }

    /// 将上下文命令非阻塞地放入有界队列。
    ///
    /// 入队失败会使连接状态失效，以免调用方继续依赖可能不同步的上下文。
    pub fn context_command(&self, command: ContextCommand) -> Result<(), CommandError> {
        // 焦点/上下文变化（切窗口、切页面、开关输入上下文）后，引擎往往要重建会话，
        // 紧接着的几个按键最容易超时 → 先置位，等一次成功应答再恢复。
        self.push.slow.store(true, Ordering::Release);
        self.reset_layout();
        let result = match &self.commands {
            None => Err(CommandError::NotStarted),
            Some(sender) => sender
                .try_send(RpcCommand::Context(command))
                .map_err(|e| match e {
                    tokio::sync::mpsc::error::TrySendError::Full(_) => CommandError::Full,
                    tokio::sync::mpsc::error::TrySendError::Closed(_) => CommandError::Closed,
                }),
        };
        if result.is_err() {
            self.push.invalidate();
        }
        result
    }

    /// 设置接收异步更新通知的窗口句柄；0 表示清除目标窗口。
    pub fn set_update_window(&self, hwnd: usize) {
        self.push.window.store(hwnd, Ordering::Release);
    }

    /// 删除不再存活的上下文对应的已排队响应。
    pub fn retain_context_updates(&self, contexts: &[u64]) {
        if let Ok(mut updates) = self.push.updates.lock() {
            updates.retain(|response| {
                response
                    .token
                    .as_ref()
                    .is_some_and(|token| contexts.contains(&token.context_id))
            });
        }
    }

    /// 取走指定上下文当前连接 epoch 的响应，并保留其他上下文的响应。
    ///
    /// 过期响应会一并清除；锁中毒时返回空集合，不尝试恢复锁状态。
    pub fn take_context_updates(&self, context_id: u64) -> Vec<KeyEventResponse> {
        let Ok(mut updates) = self.push.updates.lock() else {
            return Vec::new();
        };
        let epoch = self.push.current_epoch();
        let mut selected = Vec::new();
        updates.retain(|response| {
            let Some(token) = response.token.as_ref() else {
                return false;
            };
            if epoch == 0 || token.connection_epoch != epoch {
                return false;
            }
            if token.context_id == context_id {
                selected.push(response.clone());
                false
            } else {
                true
            }
        });
        selected
    }

    #[cfg(test)]
    /// 取走当前 epoch 的全部响应，仅供单元测试使用。
    pub fn take_updates(&self) -> Vec<KeyEventResponse> {
        match self.push.updates.lock() {
            Ok(mut updates) => {
                let epoch = self.push.current_epoch();
                std::mem::take(&mut *updates)
                    .into_iter()
                    .filter(|response| {
                        epoch != 0
                            && response
                                .token
                                .as_ref()
                                .is_some_and(|token| token.connection_epoch == epoch)
                    })
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    /// 尽力排队发送诊断日志；未启动或队列已满时静默丢弃。
    pub fn log(&self, level: impl Into<String>, text: impl Into<String>) {
        let Some(commands) = self.commands.as_ref() else {
            return;
        };
        let _ = commands.try_send(RpcCommand::Log {
            level: level.into(),
            text: text.into(),
        });
    }

    /// 合并发布布局状态，只保留最新值且不占用命令队列。
    ///
    /// 未启动或没有有效连接时忽略更新；相同值不会唤醒订阅者。
    pub fn send_layout_update(&self, update: LayoutUpdate) {
        if self.commands.is_none() || self.push.current_epoch() == 0 {
            return;
        }
        self.push.layout.0.send_if_modified(|latest| {
            if latest.as_ref() == Some(&update) {
                return false;
            }
            *latest = Some(update);
            true
        });
    }

    /// 清除待发送布局值，用于上下文切换时撤销旧状态。
    pub fn reset_layout(&self) {
        self.push.layout.0.send_replace(None);
    }

    /// 停止线程并释放其资源；多次调用安全。
    ///
    /// 先撤销窗口通知并发出停止信号，再等待原生线程最多 `STOP_TIMEOUT_MS`。
    /// 超时时不终止仍可能执行代码的线程，并故意保留模块租约以阻止 DLL 卸载；
    /// 随后换用新的共享状态，避免旧线程持锁或迟到写入污染重启后的工作代际。
    pub fn stop(&mut self) {
        self.push.window.store(0, Ordering::Release);
        self.commands.take();
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }

        if let Some(thread) = self.thread.take() {
            // Wait for native termination, including TLS destructors. Rust's
            // is_finished() can become true before the native thread exits.
            // Do not pump COM/window messages in this partially torn-down state.
            let result = unsafe {
                crate::bindings::WaitForSingleObject(
                    crate::bindings::HANDLE(thread.as_raw_handle()),
                    STOP_TIMEOUT_MS,
                )
            };
            if result == crate::bindings::WAIT_OBJECT_0 as u32 {
                let _ = thread.join();
            } else {
                // Never terminate the thread or let DllCanUnloadNow permit
                // unloading code that it may still execute. Leaking this tiny
                // lease is deliberate: even a reaper thread would itself need
                // protection through its native thread-exit epilogue.
                if let Some(module) = self.module.take() {
                    std::mem::forget(module);
                }
                drop(thread);
            }
        }
        // A timed-out worker may hold the old queue lock or publish late data.
        // Never wait for that lock, and never reuse its state on restart.
        self.push = Arc::default();
        self.module.take();
    }
}

impl Default for RpcWorker {
    /// 创建尚未启动、无有效连接的工作线程控制器。
    fn default() -> Self {
        Self {
            stop: None,
            thread: None,
            commands: None,
            push: Arc::default(),
            module: None,
        }
    }
}

impl Drop for RpcWorker {
    /// 对象销毁时执行有界停止流程。
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use weasel_common::{
        message::{Envelope, Pong, envelope::Payload},
        rpc::{RpcConnection, RpcServer},
    };

    #[allow(async_fn_in_trait)]
    trait TestConnectionProtocol {
        async fn send_pong(&self, request_id: u64, text: &str) -> Result<(), RpcError>;
        async fn send_key_event_response(
            &self,
            request_id: u64,
            response: KeyEventResponse,
        ) -> Result<(), RpcError>;
    }

    impl TestConnectionProtocol for RpcConnection {
        async fn send_pong(&self, request_id: u64, text: &str) -> Result<(), RpcError> {
            self.send(&Envelope {
                request_id,
                payload: Some(Payload::Pong(Pong { text: text.into() })),
            })
            .await
        }

        async fn send_key_event_response(
            &self,
            request_id: u64,
            response: KeyEventResponse,
        ) -> Result<(), RpcError> {
            self.send(&Envelope {
                request_id,
                payload: Some(Payload::KeyEventResponse(response)),
            })
            .await
        }
    }

    #[test]
    fn shared_worker_preserves_other_context_replies() {
        let worker = RpcWorker::default();
        worker.push.epoch.store(1, Ordering::Release);
        let a = update(1);
        let mut b = a.clone();
        b.token.as_mut().unwrap().context_id = 2;
        assert!(enqueue_update(&worker.push, 1, a));
        assert!(enqueue_update(&worker.push, 1, b));
        assert_eq!(worker.take_context_updates(1).len(), 1);
        assert_eq!(worker.take_context_updates(2).len(), 1);
        assert!(worker.take_updates().is_empty());
    }

    #[test]
    fn context_command_reports_queue_failure_and_invalidates_connection() {
        let mut worker = RpcWorker::default();
        assert_eq!(
            worker.context_command(ContextCommand::default()),
            Err(CommandError::NotStarted)
        );
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        worker.commands = Some(Arc::new(tx));
        assert_eq!(worker.context_command(ContextCommand::default()), Ok(()));
        worker.push.epoch.store(42, Ordering::Release);
        assert_eq!(
            worker.context_command(ContextCommand::default()),
            Err(CommandError::Full)
        );
        assert_eq!(worker.connection_epoch(), 0);
        drop(rx);
        assert_eq!(
            worker.context_command(ContextCommand::default()),
            Err(CommandError::Closed)
        );
    }

    #[test]
    fn layout_flood_retains_latest_without_using_command_queue() {
        let mut worker = RpcWorker::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        worker.commands = Some(Arc::new(tx));
        worker.push.epoch.store(1, Ordering::Release);
        let mut changes = worker.push.layout.0.subscribe();
        let mut last = LayoutUpdate::default();
        for x in 0..10000 {
            last.anchor = Some(weasel_common::message::RenderRect {
                left: x,
                ..Default::default()
            });
            worker.send_layout_update(last.clone());
        }
        assert!(rx.try_recv().is_err());
        assert_eq!(*changes.borrow_and_update(), Some(last.clone()));
        worker.send_layout_update(last);
        assert!(!changes.has_changed().unwrap());
        worker.reset_layout();
        assert!(changes.borrow_and_update().is_none());
    }

    fn translated_key() -> InputKey {
        InputKey {
            keycode: b'a' as i32,
            virtual_key: 0x41,
            token: Some(weasel_common::message::ContextToken {
                context_id: 1,
                connection_epoch: 1,
                generation: 1,
            }),
            ..Default::default()
        }
    }

    async fn open_input(connection: &weasel_common::rpc::RpcConnection) {
        use weasel_common::message::{Envelope, InputOpened, envelope::Payload};
        let open = connection.recv().await.unwrap().unwrap();
        let Some(Payload::OpenInput(open_input)) = open.payload else {
            panic!("expected OpenInput")
        };
        let token = open_input.token.as_ref().unwrap();
        assert!(token.context_id != 0 && token.connection_epoch != 0 && token.generation != 0);
        connection
            .send(&Envelope {
                request_id: open.request_id,
                payload: Some(Payload::InputOpened(InputOpened {
                    token: open_input.token,
                })),
            })
            .await
            .unwrap();
    }

    #[test]
    fn identity_failure_stays_in_english_without_starting_a_worker() {
        let mut worker = RpcWorker::default();
        worker.start_resolved(Err(std::io::Error::other("identity unavailable")));
        assert!(worker.thread.is_none());
        assert!(worker.commands.is_none());
        assert!(worker.module.is_none());
        assert_eq!(worker.connection_epoch(), 0);
        assert!(worker.connection_failed());
        assert!(worker.process_key_event(translated_key()).is_none());
    }

    fn update(epoch: u64) -> KeyEventResponse {
        KeyEventResponse {
            token: Some(weasel_common::message::ContextToken {
                context_id: 1,
                connection_epoch: epoch,
                generation: 1,
            }),
            revision: 1,
            ..Default::default()
        }
    }

    #[test]
    fn host_timeout_rejects_late_reply_even_when_runtime_does_not_run() {
        let mut worker = RpcWorker::default();
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
        worker.commands = Some(Arc::new(commands));
        worker.push.epoch.store(42, Ordering::Release);
        assert!(worker.process_key_event(translated_key()).is_none());
        assert_eq!(worker.connection_epoch(), 0);
        assert!(worker.connection_failed());
        // The runtime was not polled, so its deadline never fired.
        let RpcCommand::KeyEvent { response, .. } = receiver.try_recv().unwrap() else {
            panic!("expected key")
        };
        assert!(response.send(Some(update(42))).is_err());
        assert!(enqueue_update(&worker.push, 42, update(42)));
        assert!(worker.take_updates().is_empty());
        // A handshake racing cancellation cannot resurrect its old generation.
        worker.push.epoch.store(43, Ordering::Release);
        assert_eq!(worker.connection_epoch(), 0);
    }

    #[test]
    fn reconnect_discards_old_subscription_and_queued_updates() {
        let worker = RpcWorker::default();
        worker.push.epoch.store(10, Ordering::Release);
        assert!(enqueue_update(&worker.push, 10, update(10)));
        worker.push.invalidate();
        worker.push.connection_generation.store(
            worker.push.cancellation.load(Ordering::Acquire),
            Ordering::Release,
        );
        worker.push.epoch.store(11, Ordering::Release);
        assert!(enqueue_update(&worker.push, 10, update(10)));
        assert!(enqueue_update(&worker.push, 11, update(11)));
        let updates = worker.take_updates();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].token.as_ref().unwrap().connection_epoch, 11);
    }

    #[test]
    fn worker_lease_is_owned_until_join_even_after_thread_finishes() {
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(String::new());
        assert!(worker.module.is_some());
        assert!(!crate::module::can_unload());
        worker.commands.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.thread.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert!(worker.module.is_some());
        assert!(!crate::module::can_unload());
        worker.stop();
        assert!(worker.thread.is_none());
        assert!(worker.module.is_none());
    }

    #[tokio::test]
    async fn dropping_connection_releases_the_subscription_client_clone() {
        let name = format!(r"\\.\pipe\weaselnext-tip-subscription-{}", std::process::id());
        let listener = RpcServer::new(name.clone());
        let _ = tokio::time::timeout(Duration::from_millis(5), listener.accept()).await;
        let (finish, finished) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let connection = listener.accept().await.unwrap();
            let ping = connection.recv().await.unwrap().unwrap();
            connection
                .send_pong(ping.request_id, "ready")
                .await
                .unwrap();
            let _ = finished.await;
        });
        let target = Arc::new(PushTarget::default());
        let weak = Arc::downgrade(&target);
        let connection =
            tokio::time::timeout(Duration::from_secs(2), connect_client(&name, &target))
                .await
                .unwrap()
                .unwrap();
        // Ensure the subscriber actually owns its clone before exercising Drop.
        tokio::task::yield_now().await;
        drop(connection);
        drop(target);
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("old subscriber retained its target/client after connection drop");
        let _ = finish.send(());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_handshake_never_publishes_backend_availability() {
        let name = format!(
            r"\\.\pipe\weaselnext-tip-handshake-generation-{}",
            std::process::id()
        );
        let listener = RpcServer::new(name.clone());
        let _ = tokio::time::timeout(Duration::from_millis(5), listener.accept()).await;
        let (received, ping_received) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let connection = listener.accept().await.unwrap();
            let ping = connection.recv().await.unwrap().unwrap();
            received.send(()).unwrap();
            released.await.unwrap();
            connection
                .send_pong(ping.request_id, "ready")
                .await
                .unwrap();
            let _ = finished.await;
        });
        let target = Arc::new(PushTarget::default());
        let connecting_target = target.clone();
        let connecting =
            tokio::spawn(async move { connect_client(&name, &connecting_target).await });
        tokio::time::timeout(Duration::from_secs(2), ping_received)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(target.current_epoch(), 0);
        target.invalidate();
        release.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(2), connecting)
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        assert_eq!(target.current_epoch(), 0);
        assert!(target.connection_failed.load(Ordering::Acquire));
        let _ = finish.send(());
        server.await.unwrap();
    }

    #[test]
    fn idle_worker_is_unknown_not_a_connection_failure() {
        let mut worker = RpcWorker::default();
        assert_eq!(worker.connection_epoch(), 0);
        assert!(!worker.connection_failed());
        worker.push.connection_failed.store(true, Ordering::Release);
        assert!(worker.connection_failed());
        worker.stop();
        assert!(!worker.connection_failed());
    }

    #[test]
    fn key_deadline_is_not_renewed_when_dequeued() {
        let (response, _receiver) = mpsc::channel();
        let deadline = Instant::now() - COMMAND_TIMEOUT;
        let command = RpcCommand::KeyEvent {
            event: translated_key(),
            response,
            deadline,
        };
        assert_eq!(command.deadline(), deadline);
    }

    #[tokio::test]
    async fn expired_key_is_dropped_without_connecting() {
        let push = Arc::new(PushTarget::default());
        // An unchanged sentinel verifies that the expired command skips even
        // connection cleanup, as well as replying without network work.
        push.epoch.store(42, Ordering::Release);
        let state = WorkerState {
            pipe_name: String::new(),
            client: None,
            push: push.clone(),
        };
        let (sender, commands) = tokio::sync::mpsc::channel(1);
        let (_stop, stop_receiver) = tokio::sync::oneshot::channel();
        let (response, receiver) = mpsc::channel();
        sender
            .try_send(RpcCommand::KeyEvent {
                event: translated_key(),
                response,
                deadline: Instant::now() - COMMAND_TIMEOUT,
            })
            .unwrap();
        drop(sender);
        state.run(commands, stop_receiver).await;
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(push.epoch.load(Ordering::Acquire), 42);
    }

    #[tokio::test]
    async fn idle_blur_and_layout_do_not_open_a_connection() {
        let name = format!(r"\\.\pipe\weaselnext-tip-idle-{}", std::process::id());
        let listener = RpcServer::new(name.clone());
        // Keep a listening instance available: an accidental handshake would
        // hang dispatch and fail the timeout below.
        let _ = tokio::time::timeout(Duration::from_millis(5), listener.accept()).await;
        let mut state = WorkerState {
            pipe_name: name,
            client: None,
            push: Arc::default(),
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            for action in [ContextAction::Blur] {
                state
                    .dispatch(RpcCommand::Context(ContextCommand {
                        action: action as i32,
                        ..Default::default()
                    }))
                    .await;
            }
            state
                .dispatch(RpcCommand::LayoutUpdate(Default::default()))
                .await;
        })
        .await
        .unwrap();
        assert!(state.client.is_none());
        assert_eq!(state.push.epoch.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn focus_queries_real_mode_without_sending_a_key_or_editing_text() {
        use weasel_common::message::envelope::Payload;
        for ascii in [false, true] {
            let name = format!(r"\\.\pipe\weasel-focus-mode-{}-{ascii}", std::process::id());
            let listener = RpcServer::new(&name);
            let _ = tokio::time::timeout(Duration::from_millis(5), listener.accept()).await;
            let server = tokio::spawn(async move {
                let connection = listener.accept().await.unwrap();
                let ping = connection.recv().await.unwrap().unwrap();
                assert!(matches!(ping.payload, Some(Payload::Ping(_))));
                connection
                    .send_pong(ping.request_id, "ready")
                    .await
                    .unwrap();
                open_input(&connection).await;
                let request = connection.recv().await.unwrap().unwrap();
                let Some(Payload::ContextCommand(command)) = request.payload else {
                    panic!("expected Focus, not a key event");
                };
                assert_eq!(command.action, ContextAction::Focus as i32);
                connection
                    .send_key_event_response(
                        request.request_id,
                        KeyEventResponse {
                            token: command.token,
                            revision: 1,
                            ascii_mode: Some(ascii),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
                connection
            });
            let push = Arc::new(PushTarget::default());
            let mut state = WorkerState {
                pipe_name: name,
                client: None,
                push: push.clone(),
            };
            tokio::time::timeout(Duration::from_secs(2), async {
                state
                    .dispatch(RpcCommand::Context(ContextCommand {
                        token: translated_key().token,
                        action: ContextAction::Focus as i32,
                        ascii_mode: None,
                    }))
                    .await;
                while push.updates.lock().unwrap().is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let updates = push.updates.lock().unwrap().clone();
            assert_eq!(updates.len(), 1);
            assert_eq!(updates[0].ascii_mode, Some(ascii));
            assert_eq!(
                updates[0].token.unwrap().connection_epoch,
                push.current_epoch()
            );
            assert!(!updates[0].state_updated);
            assert!(updates[0].commit_text.is_empty());
            state.client.take().unwrap().close().await;
            drop(server.await.unwrap());
        }
    }

    #[tokio::test]
    async fn stalled_focus_is_bounded_and_invalidates_mode_for_english_fallback() {
        let name = format!(r"\\.\pipe\weasel-focus-stall-{}", std::process::id());
        let listener = RpcServer::new(&name);
        let _ = tokio::time::timeout(Duration::from_millis(5), listener.accept()).await;
        let push = Arc::new(PushTarget::default());
        let state = WorkerState {
            pipe_name: name,
            client: None,
            push: push.clone(),
        };
        let (sender, commands) = tokio::sync::mpsc::channel(1);
        let (_stop, stopped) = tokio::sync::oneshot::channel();
        sender
            .try_send(RpcCommand::Context(ContextCommand {
                token: translated_key().token,
                action: ContextAction::Focus as i32,
                ascii_mode: None,
            }))
            .unwrap();
        drop(sender);
        tokio::time::timeout(Duration::from_secs(1), state.run(commands, stopped))
            .await
            .unwrap();
        assert!(push.connection_failed.load(Ordering::Acquire));
        assert_eq!(push.current_epoch(), 0);
        assert!(push.updates.lock().unwrap().is_empty());
    }

    #[test]
    fn key_timeout_invalidates_connection_before_english_fallback() {
        let name = format!(r"\\.\pipe\weaselnext-key-timeout-{}", std::process::id());
        let server_name = name.clone();
        let (ready, wait_ready) = mpsc::channel();
        let server = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = RpcServer::new(server_name);
                let _ = tokio::time::timeout(Duration::from_millis(5), listener.accept()).await;
                ready.send(()).unwrap();
                let connection = listener.accept().await.unwrap();
                let ping = connection.recv().await.unwrap().unwrap();
                connection
                    .send_pong(ping.request_id, "ready")
                    .await
                    .unwrap();
                open_input(&connection).await;
                let key = connection.recv().await.unwrap().unwrap();
                let token = match key.payload {
                    Some(weasel_common::message::envelope::Payload::KeyEvent(key)) => key.token,
                    _ => panic!("expected key"),
                };
                assert!(token.as_ref().unwrap().connection_epoch > 0);
                // 首键宽限已放宽到 500ms：这里必须睡更久，才能继续验证"超时→英文回退"。
                tokio::time::sleep(Duration::from_millis(700)).await;
                let _ = connection
                    .send_key_event_response(
                        key.request_id,
                        KeyEventResponse {
                            token,
                            revision: 1,
                            eaten: true,
                            commit_text: "late".into(),
                            ..Default::default()
                        },
                    )
                    .await;
            });
        });
        wait_ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(name);
        assert!(worker.process_key_event(translated_key()).is_none());
        assert_eq!(worker.connection_epoch(), 0);
        worker.stop();
        server.join().unwrap();
    }

    // Use an isolated pipe; never load a TIP into an application or start Rime.
    fn stalled_server(
        name: String,
    ) -> (
        mpsc::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
        JoinHandle<()>,
    ) {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (received_tx, received_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let thread = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let server = RpcServer::new(name);
                // Poll accept once so the first pipe instance exists.
                let _ = tokio::time::timeout(Duration::from_millis(5), server.accept()).await;
                ready_tx.send(()).unwrap();
                let connection = server.accept().await.unwrap();
                connection.recv().await.unwrap().unwrap();
                received_tx.send(()).unwrap();
                let _ = finish_rx.await;
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        (received_rx, finish_tx, thread)
    }

    #[test]
    fn stop_cancels_stalled_handshake() {
        let name = format!(r"\\.\pipe\weaselnext-tip-stop-{}", std::process::id());
        let (received, finish, server) = stalled_server(name.clone());
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(name);
        worker.log("debug", "connect");
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        worker.stop();
        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = finish.send(());
        server.join().unwrap();
    }

    #[test]
    fn stop_times_out_without_waiting_for_worker_or_its_queue_lock() {
        let mut worker = RpcWorker::default();
        let old_push = worker.push.clone();
        old_push.window.store(123, Ordering::Release);
        let background_push = old_push.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        worker.module = Some(crate::module::ModuleLease::new());
        worker.thread = Some(thread::spawn(move || {
            let _queue = background_push.updates.lock().unwrap();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            // Simulate an old worker publishing state after stop/restart.
            background_push.epoch.store(99, Ordering::Release);
            let _ = finished_tx.send(());
        }));
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        worker.stop();
        let elapsed = started.elapsed();
        // Release before asserting, so failures also allow test cleanup.
        let _ = release_tx.send(());
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(elapsed < Duration::from_secs(1));
        assert!(worker.thread.is_none());
        assert!(worker.module.is_none());
        assert!(!crate::module::can_unload());
        assert_eq!(old_push.window.load(Ordering::Acquire), 0);
        assert!(!Arc::ptr_eq(&worker.push, &old_push));
        assert_eq!(worker.connection_epoch(), 0);
        assert!(worker.take_updates().is_empty());
        worker.stop(); // Idempotent, with no retained thread to wait for.
    }

    #[test]
    fn worker_reconnects_after_server_disconnect() {
        let name = format!(r"\\.\pipe\weaselnext-tip-reconnect-{}", std::process::id());
        let pipe = name.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let server = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                tokio::time::timeout(Duration::from_secs(3), async move {
                    let listener = RpcServer::new(pipe);
                    let _ = tokio::time::timeout(Duration::from_millis(5), listener.accept()).await;
                    ready_tx.send(()).unwrap();
                    // First connection dies during handshake (deployment).
                    let old = listener.accept().await.unwrap();
                    old.recv().await.unwrap().unwrap();
                    drop(old);
                    let fresh = listener.accept().await.unwrap();
                    let ping = fresh.recv().await.unwrap().unwrap();
                    fresh
                        .send_pong(ping.request_id, "new server")
                        .await
                        .unwrap();
                    open_input(&fresh).await;
                    let key = fresh.recv().await.unwrap().unwrap();
                    let Some(weasel_common::message::envelope::Payload::KeyEvent(event)) =
                        key.payload
                    else {
                        panic!("expected key")
                    };
                    fresh
                        .send_key_event_response(
                            key.request_id,
                            KeyEventResponse {
                                eaten: true,
                                token: event.token,
                                revision: 1,
                                ..Default::default()
                            },
                        )
                        .await
                        .unwrap();
                    let _ = finish_rx.await;
                })
                .await
                .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(name);
        assert!(worker.process_key_event(translated_key()).is_none());
        assert!(worker.connection_failed());
        assert!(worker.process_key_event(translated_key()).unwrap().eaten);
        assert!(!worker.connection_failed());
        worker.stop();
        let _ = finish_tx.send(());
        server.join().unwrap();
    }

    #[test]
    fn stalled_handshake_does_not_block_key_fallback_or_stop() {
        let name = format!(r"\\.\pipe\weaselnext-tip-timeout-{}", std::process::id());
        let (received, finish, server) = stalled_server(name.clone());
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(name);
        let started = Instant::now();
        assert!(worker.process_key_event(translated_key()).is_none());
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.stop();
        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = finish.send(());
        server.join().unwrap();
    }
}
