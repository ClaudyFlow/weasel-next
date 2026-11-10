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
        mpsc::{self, RecvTimeoutError, Sender},
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
///
/// ⚠️ 2026-11-08 17:5x：从 80ms 放宽到 180ms。原因有实测数据：
/// 0.18.7 起 `apply_commit` 在**提交路径同步跑双神经预测**（key+desk），
/// 引擎侧实测一次提交键 **70–108ms**（探针 `probe_commit_latency`，12 轮）。
/// 80ms 的预算下，worker 在 `dispatch` 返回后一律判过期：
/// `if Instant::now() >= deadline { drop(response) }` —— 迟到的响应被**直接丢掉**，
/// 宿主拿不到 `commit_text`，这一键按「没吃下」退回应用。
/// 用户看到的正是「打字按数字选候选时有概率上数字而不是候选」：
/// 引擎其实已经选好候选并提交，但响应被 80ms 判死，数字就透传给了应用。
///
/// 180ms 能盖住 108ms 实测峰值 + RPC 往返；宿主侧总等待 = 180 + 240 = 420ms
/// （2026-11-08 19:xx 把余量 60→240，见 [`HOST_WAIT_MARGIN`]），
/// 只有真慢的键会等满，普通按键仍然 5–20ms 返回。
const COMMAND_TIMEOUT: Duration = Duration::from_millis(180);
/// 冷启动 / 刚切到某程序时的**首个按键**要顺带完成"连接 + 握手 + open_input"。
///
/// 这里刻意**不**用 1200ms 去盖住引擎冷启动（实测 broker 唤醒到 `librime
/// initialized` 要 ~1010ms）：单键堵满一秒会让用户每个键都感觉「卡住」，而
/// 冷启动期间的后端本来也回不来。现在把唤醒放后台，单键最多等 500ms；
/// 期间处于组字态时这一键由 TIP 吞掉（见 `key_event::timeout_fallback`），
/// 不再把编码透传给宿主，下一键重建连接后由引擎返回完整组合串重新同步。
const CONNECT_KEY_TIMEOUT: Duration = Duration::from_millis(500);
/// 两次唤醒之间的冷却；后端确实起不来时不至于每个按键都去戳一次 broker。
const WAKE_COOLDOWN: Duration = Duration::from_millis(1500);
/// 工作线程重建冷却：线程反复死亡时避免每个按键都重建一次。
const WORKER_RESTART_COOLDOWN: Duration = Duration::from_millis(3000);
/// 后端**可能正在忙**时的按键预算（引擎刚重启、刚切上下文/页面、刚切中英）。
///
/// 这些时刻 RPC 连接还在，走不到上面的 `CONNECT_KEY_TIMEOUT` 分支；但后端要重建会话
/// 或等一次布局往返，80ms 常常不够 → 前几个字母被英文直通给应用。按键已经被输入法
/// 吞下，多等一会儿只是晚几十毫秒出现，不会漏给应用，所以这里统一放宽。
const BUSY_KEY_TIMEOUT: Duration = Duration::from_millis(1000);
/// 宿主线程等待按键结果时比预算多留的余量，避免在边界上误判超时。
///
/// ⚠️ 2026-11-08 19:xx 60→240ms。实测证据（`key-latency.csv`，budget=180 段）：
/// 切方案那一键（`Ctrl+`` 展开菜单 + 数字确认）冷缓存时引擎侧可达 **273–328ms**
/// （`Weasel-Next-Engine-key-latency.csv` 19:04:42–43 三连尖峰）。宿主 180+60=240ms
/// 就先放弃，于是它返回 BOOL(0)，用户看到的正是「卡一下 → 数字/空格上屏 →
/// 才切换方案」。
///
/// 240ms 余量 ⇒ 普通键宿主最多等 `180+240=420ms`（只有真慢的键会等满；正常 1–5ms
/// 返回），足以盖住「上一键引擎里还有 ≤330ms 残活 + 本键处理」的最坏情况。
/// 代价是引擎真挂住时单键最多堵 420ms —— 但比把数字漏进应用可接受。
///
/// ⚠️ 这个余量同时是 **worker 派发截止时间的唯一口径**（见 [`RpcCommand::host_deadline`]）：
/// 旧实现给排队键追加 `MIN_DISPATCH_BUDGET`(300ms) 的宽限，比宿主 420ms 先放弃，
/// 结果 `dispatch()` 末尾照样丢响应 —— 两边必须对齐。
const HOST_WAIT_MARGIN: Duration = Duration::from_millis(240);
/// 停止工作线程时等待原生线程退出的最长毫秒数。
const STOP_TIMEOUT_MS: u32 = 100;

/// 抄自 msime `DeferredKeyFailurePolicy::IsCandidateNavigationOnlyKey` 的 Win32 VK 判据。
/// 这些键的回复**只是 ack**（翻页或移动高亮），永远不会提交文本：
/// 所以它们的超时是 soft miss，不该像传输失败那样拆连接、清待处理队列。
/// Left/Right 刻意不在清单里（与 msime 一致）：它们会在编码内移动光标，依赖会话状态。
const VK_TAB: u32 = 0x09;
const VK_PRIOR: u32 = 0x21; // PageUp
const VK_NEXT: u32 = 0x22; // PageDown
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;

/// 是否是被吃键后只回 ack 的候选导航键。
pub(crate) fn is_candidate_navigation_only_key(virtual_key: u32) -> bool {
    matches!(
        virtual_key,
        VK_TAB | VK_PRIOR | VK_NEXT | VK_UP | VK_DOWN
    )
}

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

/// 按键结果的两类归宿。分开表示才能把「导航键 soft miss」和「引擎没应答」
/// 区分开：前者保留连接与待处理队列，后者由 worker 自行隔离故障。
enum KeyOutcome {
    /// 正常往返；`None` 表示引擎侧没有产出应答（worker 已自行隔离故障）。
    Reply(Option<KeyEventResponse>),
    /// 导航键超时（Tab/PageUp/PageDown/Up/Down）：只翻页/移高亮，不提交文本。
    /// 保留连接，这一键丢掉即止，绝不重发。
    SoftMiss,
}

/// 私有 Tokio 运行时串行处理的工作项。
enum RpcCommand {
    /// 尽力发送诊断日志，不等待服务器确认。
    Log { level: String, text: String },
    /// 处理需在宿主线程等待的按键，并携带不可续期的截止时间。
    KeyEvent {
        event: InputKey,
        response: Sender<KeyOutcome>,
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
    // 握手成功即视为后端已恢复。**必须在这里清 `failed`**：它是"回复投递失败"
    // 留下的闩锁，旧实现没有任何清除点，一旦在 broker/引擎重启的瞬间置位，
    // 该宿主进程内的输入法就会永久失效（只有重启宿主进程才能恢复）。
    target.failed.store(false, Ordering::Release);
    target.connection_failed.store(false, Ordering::Release);
    target.slow.store(false, Ordering::Release);
    target
        .connection_generation
        .store(generation, Ordering::Release);
    target.epoch.store(epoch, Ordering::Release);
    // 重建连接后主动唤醒宿主一次，让它按新连接的（空）状态重画一次编码栏/候选窗，
    // 而不是继续显示断连前那份陈旧快照。
    target.notify();
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

    /// 宿主**放弃等待**这条命令的绝对时刻。
    ///
    /// 按键入队的 deadline 是 `入队时刻 + budget`；宿主在 `recv_timeout` 里多等
    /// [`HOST_WAIT_MARGIN`]，所以它的耐心界线是 `deadline + HOST_WAIT_MARGIN`。
    /// 派发截止时间必须用这个值：早于它判死会在宿主还在等的时候丢响应（数字/空格
    /// 透传的直接原因）；晚于它继续处理则结果没人要，还会白踢一条健康连接。
    fn host_deadline(&self) -> Instant {
        match self {
            Self::KeyEvent { .. } => self.deadline() + HOST_WAIT_MARGIN,
            _ => self.deadline(),
        }
    }

    /// 这条命令是否是「回复只是 ack」的候选导航键。
    fn is_navigation_only(&self) -> bool {
        matches!(
            self,
            Self::KeyEvent { event, .. } if is_candidate_navigation_only_key(event.virtual_key)
        )
    }

    /// 把（可能被宽限过的）截止时间写回命令；非按键命令原样返回。
    ///
    /// 2026-11-08 19:xx：`run()` 原来为「排队被拖过期」的按键算出
    /// `arrival + MIN_DISPATCH_BUDGET`(300ms)，但那个宽限比宿主耐心(420ms)短，
    /// `dispatch()` 里两处 `Instant::now() >= deadline` 照样会在宿主等待期间判死。
    /// 现在写回的是宿主的放弃时刻（[`host_deadline`]），两边同进同退。
    fn with_deadline(self, new_deadline: Instant) -> Self {
        match self {
            Self::KeyEvent {
                event, response, ..
            } => Self::KeyEvent {
                event,
                response,
                deadline: new_deadline,
            },
            other => other,
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
    /// 上一次为唤醒后端而戳 broker 的时间；用于冷却，避免每个按键都去戳一次。
    last_wake: Instant,
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
            // 派发截止时间只认**宿主的耐心**（`入队 budget + HOST_WAIT_MARGIN`）。
            //
            // 2026-11-08 19:xx 两个实测把这里逼定成这个口径：
            // * 切方案键引擎侧冷态 273–328ms ⇒ 宿主必须等到 420ms 才收得到响应；
            // * 旧实现给排队键追加入队后 300ms（`MIN_DISPATCH_BUDGET`），比宿主先
            //   放弃 ⇒ `dispatch()` 末尾照样丢响应、数字/空格照样漏。
            // 若出队时宿主已经放弃（deadline 已过），`dispatch()` 开头会直接丢弃：
            // 不连接、不处理、不拆连接（处理了也没人要，拆连接反而害下一键重连）。
            let deadline = command.host_deadline();
            // 宽限必须真正落到命令上，否则 `dispatch()` 仍按入队时的 deadline 判死。
            let command = command.with_deadline(deadline);
            // 导航键 soft miss：保留一个 sender 克隆。若 `dispatch()` 被截止时间
            // 取消，它自己的 sender 会随 future 一起消失；没有这个克隆，宿主会
            // 把「sender 静默消失」误判成传输失败而强拆连接。这里显式投递 SoftMiss。
            let soft_miss = command.is_navigation_only();
            let soft_reply = match &command {
                RpcCommand::KeyEvent { response, .. } if soft_miss => Some(response.clone()),
                _ => None,
            };
            self.discard_disconnected_client().await;

            // 建立连接时的首键：放宽预算，避免这一键被英文回退吞掉；
            // 连接就绪后仍按常规预算，Await 串行以保证按键回复与上下文命令的顺序。
            let establishing = self.client.is_none();
            // 注意：入队时可能还没置位 slow（例如刚切上下文那一瞬），所以这里再兜一层
            let busy = self.push.slow.load(Ordering::Acquire);
            let dispatch_deadline = if establishing || busy {
                // 建连/忙时 worker 自己需要宽预算，但**不能超过宿主耐心**——
                // 否则结果送到时宿主早已 invalidate 掉 epoch，等于白发。
                (Instant::now() + CONNECT_KEY_TIMEOUT.max(BUSY_KEY_TIMEOUT)).min(deadline)
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
                    "command deadline exceeded",
                );
                if let Some(reply) = soft_reply {
                    // 软失误：键丢掉即止，连接、epoch、待处理队列一概不动。
                    let _ = reply.send(KeyOutcome::SoftMiss);
                    report(
                        "candidate-nav-soft-miss",
                        Some(&self.pipe_name),
                        "navigation key deadline exceeded; connection kept",
                    );
                } else {
                    // Invalidate queued racing replies before English fallback.
                    self.disconnect_invalidated_client().await;
                }
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
        if self.client.is_some() {
            return;
        }
        if let Some(client) = connect_client(&self.pipe_name, &self.push).await {
            self.client = Some(client);
            return;
        }
        // 连不上多半是"五分钟不打字 → 引擎空闲自退"。唤醒必须在后台：
        // 冷启动实测 ~1010ms，远超单键预算；同步等待只会让连续几个键
        // 各自堵满一秒，而且这段时间里的键仍会被判超时。这里只负责戳一次
        // broker，后续按键各自直连重试，连上即恢复。
        let now = Instant::now();
        if now.duration_since(self.last_wake) >= WAKE_COOLDOWN {
            self.last_wake = now;
            tokio::spawn(wake_broker());
        }
        self.push.connection_failed.store(true, Ordering::Release);
        self.push.notify();
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
        let soft_miss = command.is_navigation_only();
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
                //
                // 导航键例外：投递一个明确的 SoftMiss，让宿主知道这是软失误，
                // 而不是 sender 意外消失（后者会触发强失效）。
                if Instant::now() >= deadline {
                    if soft_miss {
                        report(
                            "candidate-nav-soft-miss",
                            Some(&self.pipe_name),
                            "expired navigation key; connection kept",
                        );
                        let _ = response.send(KeyOutcome::SoftMiss);
                    } else {
                        drop(response);
                    }
                    return;
                }
                let value = self.handle_key_event(event).await;
                // 迟到的应答或宿主已经放弃的接收方：普通键按传输/调度故障拆连接；
                // 导航键只是 soft miss（回复永远是 ack，没有丢提交文本的风险）。
                if Instant::now() >= deadline || response.send(KeyOutcome::Reply(value)).is_err() {
                    if soft_miss {
                        report(
                            "candidate-nav-soft-miss",
                            Some(&self.pipe_name),
                            "navigation key reply missed; connection kept",
                        );
                    } else {
                        self.disconnect_invalidated_client().await;
                    }
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
    /// 上一次重建工作线程的时刻，用于冷却，避免线程反复死亡时每键都重建。
    last_restart: Option<Instant>,
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
            last_wake: Instant::now(),
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

    /// 将按键提交到后台，并在宿主线程最多等待 `budget + HOST_WAIT_MARGIN` 取得结果。
    ///
    /// 入队截止时间为 `budget` 且不会在出队时续期；worker 只在宿主耐心耗尽
    /// （`deadline + HOST_WAIT_MARGIN`）时才放弃这一键，见 [`RpcCommand::host_deadline`]。
    /// 超时会在宿主线程同步使 epoch 失效，防止运行时调度延迟期间到达的响应被误用；
    /// 失败返回 `None` 供输入直通。
    pub fn process_key_event(&mut self, event: InputKey) -> Option<KeyEventResponse> {
        // 注意：这里**不**因为 `failed` 直接返回 `None`。
        //
        // 旧实现把 `failed`（回复订阅溢出/宿主投递失败留下的闩锁）当作永久禁用。
        // broker/引擎重启的那一刻若置位，之后每个键都在宿主线程立即英文直通，
        // 连一次重连都不会尝试 —— 用户看到的就是"输入法在这个窗口彻底没反应"，
        // 只能重启宿主进程。现在 `failed` 仅作诊断标记，真正的恢复由
        // `connect_client` 握手成功后清除；连接失效时 worker 会自行重建连接。
        if self.commands.is_none() {
            return None;
        }
        if self.push.failed.load(Ordering::Acquire) {
            // 保留一条证据：确实处于投递失败后的恢复路径上。
            report("key-recover", None, "delivery flagged failed, retrying");
        }
        // msime `DeferredKeyFailurePolicy`：Tab / PageUp / PageDown / Up / Down
        // 的回复只是翻页/移动高亮的 ack，永不提交文本 ⇒ 超时属于 soft miss，
        // 不拆连接、不清待处理队列。键本身丢掉即止，绝不重发。
        let navigation_only = is_candidate_navigation_only_key(event.virtual_key);
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
        // 乙-1 插桩：量的是**宿主线程被这一键堵住多久**（= 用户感觉到的卡顿）。
        let latency = crate::key_latency::scoped(budget);
        let queued = self
            .commands
            .as_ref()
            .expect("commands 已在函数开头检查")
            .try_send(RpcCommand::KeyEvent {
                event,
                response,
                deadline: Instant::now() + budget,
            });
        if let Err(error) = queued {
            report("key-queue", None, &error);
            latency.finish(crate::key_latency::Outcome::Disconnect, false);
            // 通道关闭 = worker 线程已经不在了（panic 或运行时创建失败）。
            // 旧实现只丢这一键、永不重建，于是这个宿主进程永久失去输入法。
            // 这里按冷却重建一次；`full` 只说明队列满，丢掉这一键即可。
            let closed = matches!(
                error,
                tokio::sync::mpsc::error::TrySendError::Closed(_)
            );
            if closed {
                self.restart_worker();
            }
            return None;
        }

        // 注意：recv_timeout 的 `Ok(T)` 里 T 就是 KeyOutcome，不再包一层 Option。
        // 「sender 无应答即消失」表现为 Err(Disconnected)，属于传输级故障。
        match receiver.recv_timeout(budget + HOST_WAIT_MARGIN) {
            Ok(KeyOutcome::Reply(value)) => {
                // 这一次往返是通的 → 后端恢复正常，后续按键回到常规预算
                self.push.slow.store(false, Ordering::Release);
                let eaten = value.as_ref().is_some_and(|value| value.eaten);
                latency.finish(crate::key_latency::Outcome::Ok, eaten);
                value
            }
            Ok(KeyOutcome::SoftMiss) => {
                // 导航键 soft miss：worker 明确告知「没等到 ack，但连接还在」。
                // 这一键丢掉即止，不拆连接、不清待处理队列、也不重发。
                latency.finish(crate::key_latency::Outcome::Timeout, false);
                None
            }
            Err(RecvTimeoutError::Timeout) if navigation_only => {
                // worker 还没来得及投递 SoftMiss（例如它自己的截止时间稍晚），
                // 宿主已经等满耐心。导航键按软失误处理，保留连接。
                report(
                    "candidate-nav-soft-miss",
                    None,
                    "host wait expired; connection kept",
                );
                latency.finish(crate::key_latency::Outcome::Timeout, false);
                None
            }
            Err(error) => {
                report("host-wait", None, error);
                // 通道断开（worker 线程死亡/派发被强制取消）或运行时被调度停摆：
                // 这是传输级故障，导航键也要强失效，不能伪装成 soft miss。
                self.push.invalidate();
                latency.finish(crate::key_latency::Outcome::Timeout, false);
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

    /// 命令通道关闭（工作线程已退出）时按冷却重建一次工作线程。
    ///
    /// 这里不新增常驻线程：只是替换掉已经死掉的那条。`stop()` 会把
    /// 宿主窗口句柄清零并换用新的共享状态，所以重建后要把窗口句柄挂回去，
    /// 否则新的推送无处投递，宿主永远收不到更新。
    fn restart_worker(&mut self) {
        let now = Instant::now();
        if self
            .last_restart
            .is_some_and(|last| now.duration_since(last) < WORKER_RESTART_COOLDOWN)
        {
            return;
        }
        self.last_restart = Some(now);
        let window = self.push.window.load(Ordering::Acquire);
        report("worker-restart", None, "command channel closed");
        self.start();
        self.push.window.store(window, Ordering::Release);
    }

    /// 停止线程并释放其资源；多次调用安全。
    ///
    /// 先撤销窗口通知并发出停止信号，再等待原生线程最多 `STOP_TIMEOUT_MS`。
    /// 超时时不终止仍可能执行代码的线程，并故意保留模块租约以阻止 DLL 卸载；
    /// 随后换用新的共享状态，避免旧线程持锁或迟到写入污染重启后的工作代际。
    pub fn stop(&mut self) {
        self.push.window.store(0, Ordering::Release);
        self.commands.take();
        // 乙-1：进程/上下文即将收摊，把还没落盘的按键延迟采样刷出去。
        crate::key_latency::flush_pending();
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
            last_restart: None,
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

    /// 构造 msime 清单内的导航键；`keycode` 对本次测试不重要，只保留 Win32 VK。
    fn navigation_key(virtual_key: u32) -> InputKey {
        InputKey {
            virtual_key,
            token: translated_key().token,
            ..Default::default()
        }
    }

    /// 重连期间允许丢键（生产语义：这一键英文直通），但必须在若干次内恢复。
    fn key_eaten_within(worker: &mut RpcWorker, attempts: usize) -> bool {
        (0..attempts).any(|_| {
            worker
                .process_key_event(translated_key())
                .is_some_and(|response| response.eaten)
        })
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
        assert!(response.send(KeyOutcome::Reply(Some(update(42)))).is_err());
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

    #[test]
    fn key_host_deadline_adds_the_host_wait_margin() {
        // 入队 deadline 只是 `budget`；宿主实际等到 `budget + HOST_WAIT_MARGIN`。
        // worker 必须以宿主耐心为派发截止，否则切方案键(273–328ms)会在宿主还在
        // 等的时候就丢响应 ⇒ 数字/空格透传。
        let (response, _receiver) = mpsc::channel();
        let deadline = Instant::now();
        let command = RpcCommand::KeyEvent {
            event: translated_key(),
            response,
            deadline,
        };
        assert_eq!(command.host_deadline(), deadline + HOST_WAIT_MARGIN);
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
            last_wake: Instant::now(),
        };
        let (sender, commands) = tokio::sync::mpsc::channel(1);
        let (_stop, stop_receiver) = tokio::sync::oneshot::channel();
        let (response, receiver) = mpsc::channel();
        sender
            .try_send(RpcCommand::KeyEvent {
                event: translated_key(),
                response,
                // 只有超过「入队 budget + 宿主余量」的键才算真过期（宿主早已 BOOL(0)）。
                deadline: Instant::now() - COMMAND_TIMEOUT - HOST_WAIT_MARGIN,
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
            last_wake: Instant::now(),
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
                last_wake: Instant::now(),
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
            last_wake: Instant::now(),
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
        // 后端忙时按键预算是 BUSY_KEY_TIMEOUT，等它跑完才能断言"放弃并回退"。
        tokio::time::timeout(
            BUSY_KEY_TIMEOUT + HOST_WAIT_MARGIN + Duration::from_millis(300),
            state.run(commands, stopped),
        )
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
                // 首键宽限是 CONNECT_KEY_TIMEOUT + HOST_WAIT_MARGIN：这里必须睡更久，
                // 才能继续验证"超时→英文回退"。写成算式，免得以后改超时又忘了同步。
                tokio::time::sleep(CONNECT_KEY_TIMEOUT + HOST_WAIT_MARGIN + Duration::from_millis(150)).await;
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

    /// msime `DeferredKeyFailurePolicy` 的五项清单要一字不差：多算一个键，
    /// 就可能把真正的提交失败当软失误放过去；少算一个键，则会继续"越超时越拆连接"。
    #[test]
    fn candidate_navigation_only_key_matches_msime_five_vks() {
        for vk in [VK_TAB, VK_PRIOR, VK_NEXT, VK_UP, VK_DOWN] {
            assert!(
                is_candidate_navigation_only_key(vk),
                "VK {vk:#04x} 属于 msime 的五项导航键"
            );
        }
        for vk in [0x25, 0x27, 0x41, 0x20, 0x0d] {
            assert!(
                !is_candidate_navigation_only_key(vk),
                "VK {vk:#04x} 不能被误判为导航键（Left/Right 会移动编码内光标，依赖会话状态）"
            );
        }
    }

    /// 过期的导航键必须显式投递 SoftMiss，不能让 sender 静默消失——
    /// 后者在宿主侧等价于传输故障，会触发强失效，正是本轮要根治的恶性循环。
    #[tokio::test]
    async fn expired_navigation_key_replies_soft_miss_without_invalidating() {
        let push = Arc::new(PushTarget::default());
        push.epoch.store(42, Ordering::Release);
        let mut state = WorkerState {
            pipe_name: String::new(),
            client: None,
            push: push.clone(),
            last_wake: Instant::now(),
        };
        let (response, receiver) = mpsc::channel();
        state
            .dispatch(RpcCommand::KeyEvent {
                event: navigation_key(VK_NEXT),
                response,
                deadline: Instant::now() - COMMAND_TIMEOUT - HOST_WAIT_MARGIN,
            })
            .await;
        assert!(
            matches!(receiver.try_recv(), Ok(KeyOutcome::SoftMiss)),
            "导航键过期要发 SoftMiss，不能让 sender 静默消失"
        );
        assert_eq!(push.epoch.load(Ordering::Acquire), 42, "软失误不能拆连接");
        assert!(!push.connection_failed.load(Ordering::Acquire));
    }

    /// 对照：普通键过期仍按原来的"丢 sender → 宿主强失效"处理，行为零回归。
    #[tokio::test]
    async fn expired_letter_key_still_drops_response_silently() {
        let push = Arc::new(PushTarget::default());
        push.epoch.store(42, Ordering::Release);
        let mut state = WorkerState {
            pipe_name: String::new(),
            client: None,
            push: push.clone(),
            last_wake: Instant::now(),
        };
        let (response, receiver) = mpsc::channel();
        state
            .dispatch(RpcCommand::KeyEvent {
                event: translated_key(),
                response,
                deadline: Instant::now() - COMMAND_TIMEOUT - HOST_WAIT_MARGIN,
            })
            .await;
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        assert_eq!(push.epoch.load(Ordering::Acquire), 42);
    }

    /// 宿主侧：worker 还没投递 SoftMiss 前就等满耐心，导航键也不许自己拆连接。
    #[test]
    fn host_navigation_timeout_without_worker_soft_reply_keeps_connection() {
        let mut worker = RpcWorker::default();
        let (commands, _receiver) = tokio::sync::mpsc::channel(1);
        worker.commands = Some(Arc::new(commands));
        worker.push.epoch.store(42, Ordering::Release);
        let started = Instant::now();
        assert!(worker.process_key_event(navigation_key(VK_DOWN)).is_none());
        assert!(
            started.elapsed() >= COMMAND_TIMEOUT + HOST_WAIT_MARGIN,
            "没拿到软信号时仍要等满宿主耐心（budget=180ms + margin=240ms）"
        );
        assert_eq!(worker.connection_epoch(), 42, "导航键超时不能自己拆连接");
        assert!(!worker.connection_failed());
    }

    /// 传输级失败（sender 无应答即消失）不属于 soft miss，导航键也要强失效。
    #[test]
    fn dropped_navigation_key_response_still_invalidates_connection() {
        let mut worker = RpcWorker::default();
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
        worker.commands = Some(Arc::new(commands));
        worker.push.epoch.store(42, Ordering::Release);
        let dropper = thread::spawn(move || {
            let RpcCommand::KeyEvent { response, .. } = receiver.blocking_recv().unwrap() else {
                panic!("expected key")
            };
            drop(response);
        });
        assert!(worker.process_key_event(navigation_key(VK_PRIOR)).is_none());
        dropper.join().unwrap();
        assert_eq!(worker.connection_epoch(), 0, "传输失败仍要拆连接");
        assert!(worker.connection_failed());
    }

    /// 端到端：第一键 PageDown 故意不回，第二键必须复用同一条连接。
    /// 这同时钉死「被吃键不重发」——第一键只进一次 RPC，没有重投。
    #[test]
    fn navigation_key_timeout_is_a_soft_miss_and_keeps_the_connection() {
        let name = format!(r"\\.\pipe\weaselnext-nav-soft-{}", std::process::id());
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
                // 第一键是导航键：故意不回，逼出软超时。
                let first = connection.recv().await.unwrap().unwrap();
                let Some(Payload::KeyEvent(event)) = first.payload else {
                    panic!("expected navigation key")
                };
                assert!(is_candidate_navigation_only_key(event.virtual_key));
                // 第二键正常回：必须走同一条连接，不再握手 / open_input。
                let second = connection.recv().await.unwrap().unwrap();
                let Some(Payload::KeyEvent(event)) = second.payload else {
                    panic!("expected key")
                };
                connection
                    .send_key_event_response(
                        second.request_id,
                        KeyEventResponse {
                            eaten: true,
                            token: event.token,
                            revision: 2,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            });
        });
        wait_ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(name);
        assert!(worker.process_key_event(navigation_key(VK_NEXT)).is_none());
        let kept = worker.connection_epoch();
        assert!(kept != 0, "导航键超时必须保留连接 epoch");
        assert!(!worker.connection_failed(), "导航键超时不能置连接失败");
        // 迟到的第一键应答没有走 pending，也不会污染更新队列。
        assert!(worker.take_updates().is_empty());
        let second = worker
            .process_key_event(translated_key())
            .expect("下一键应复用同一连接");
        assert!(second.eaten);
        assert_eq!(worker.connection_epoch(), kept, "不能因为软失误重建连接");
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

    /// 服务端**以同一个管道名整代重启**（旧监听与旧连接一起销毁）后，TIP 必须自愈。
    ///
    /// 这正是线上下事故的形态：broker/引擎被重启，而宿主进程里的 TIP 实例还活着。
    /// 旧测试 `worker_reconnects_after_server_disconnect` 只覆盖"同一个 listener
    /// 上换一条连接"，漏了监听器本身被销毁重建的情况。
    #[test]
    fn worker_recovers_after_server_restarts_on_same_pipe() {
        use weasel_common::message::envelope::Payload;

        let name = format!(
            r"\\.\pipe\weaselnext-tip-server-restart-{}",
            std::process::id()
        );
        let pipe = name.clone();
        let (ready1_tx, ready1_rx) = mpsc::channel();
        let (closed1_tx, closed1_rx) = mpsc::channel();
        let (ready2_tx, ready2_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let server = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                tokio::time::timeout(Duration::from_secs(20), async move {
                    // 第一代服务端：服务若干次按键后整体销毁（监听 + 连接）。
                    {
                        let listener = RpcServer::new(pipe.clone());
                        ready1_tx.send(()).unwrap();
                        let conn = loop {
                            match listener.accept().await {
                                Ok(conn) => break conn,
                                Err(_) => continue,
                            }
                        };
                        let ping = conn.recv().await.unwrap().unwrap();
                        conn.send_pong(ping.request_id, "gen1").await.unwrap();
                        open_input(&conn).await;
                        // 只服务一次按键就整代销毁。主线程拿到第一个被消费的按键后
                        // 便会等待关闭信号，若这里继续等第 2..4 个键就会双方互等。
                        if let Ok(Some(key)) = conn.recv().await {
                            if let Some(Payload::KeyEvent(event)) = key.payload {
                                conn.send_key_event_response(
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
                            }
                        }
                        drop(conn);
                        drop(listener);
                    }
                    closed1_tx.send(()).unwrap();
                    // 第二代服务端：同一个管道名重新监听。旧实例可能短暂占用名字，重试即可。
                    let listener = RpcServer::new(pipe.clone());
                    loop {
                        if listener.bind().await.is_ok() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    ready2_tx.send(()).unwrap();
                    let conn = loop {
                        match listener.accept().await {
                            Ok(conn) => break conn,
                            Err(_) => continue,
                        }
                    };
                    let ping = conn.recv().await.unwrap().unwrap();
                    conn.send_pong(ping.request_id, "gen2").await.unwrap();
                    open_input(&conn).await;
                    // 重连可能丢掉首个按键，把后续几次都答掉。
                    for _ in 0..4 {
                        let Ok(Some(key)) = conn.recv().await else {
                            break;
                        };
                        let Some(Payload::KeyEvent(event)) = key.payload else {
                            break;
                        };
                        conn.send_key_event_response(
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
                    }
                    let _ = finish_rx.await;
                })
                .await
                .unwrap();
            });
        });
        ready1_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(name);
        assert!(key_eaten_within(&mut worker, 5));
        let first_epoch = worker.connection_epoch();
        assert!(first_epoch != 0);

        closed1_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        ready2_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        // 等客户端读到 EOF 并把 epoch 置 0，等价于进程内的断连通知。
        std::thread::sleep(Duration::from_millis(200));

        assert!(key_eaten_within(&mut worker, 5));
        assert!(worker.connection_epoch() != first_epoch);
        assert!(!worker.connection_failed());

        worker.stop();
        let _ = finish_tx.send(());
        server.join().unwrap();
    }

    /// 投递失败闩锁不得让 TIP 永久失效。
    ///
    /// 旧实现里 `failed` 一旦置位就没有任何清除点，`process_key_event` 会一直
    /// 立即返回 `None`（英文直通），只有重启宿主进程才能恢复 —— 线上事故的现场。
    #[test]
    fn delivery_failure_flag_does_not_disable_input() {
        use weasel_common::message::envelope::Payload;

        let name = format!(
            r"\\.\pipe\weaselnext-tip-failed-latch-{}",
            std::process::id()
        );
        let pipe = name.clone();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = tokio::sync::oneshot::channel();
        let server = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                tokio::time::timeout(Duration::from_secs(5), async move {
                    let listener = RpcServer::new(pipe);
                    ready_tx.send(()).unwrap();
                    let conn = listener.accept().await.unwrap();
                    let ping = conn.recv().await.unwrap().unwrap();
                    conn.send_pong(ping.request_id, "ready").await.unwrap();
                    open_input(&conn).await;
                    for _ in 0..4 {
                        let Ok(Some(key)) = conn.recv().await else {
                            break;
                        };
                        let Some(Payload::KeyEvent(event)) = key.payload else {
                            break;
                        };
                        conn.send_key_event_response(
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
                    }
                    let _ = finish_rx.await;
                })
                .await
                .unwrap();
            });
        });
        ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut worker = RpcWorker::default();
        worker.start_with_pipe(name);
        // 模拟 broker 重启瞬间"回复投递失败"留下的闩锁（尚未建立连接）。
        worker.push.failed.store(true, Ordering::Release);
        assert!(key_eaten_within(&mut worker, 3));
        assert!(!worker.push.failed.load(Ordering::Acquire));
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
        // 上界就是首键预算（握手停滞时最多等到这么久），加一点调度余量。
        assert!(started.elapsed() < CONNECT_KEY_TIMEOUT + HOST_WAIT_MARGIN + Duration::from_millis(400));
        let _ = finish.send(());
        server.join().unwrap();
    }
}
