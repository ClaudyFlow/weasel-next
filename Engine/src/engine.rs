//! 管理输入会话、Rime 调用与渲染状态，并在工作线程中串行处理它们。
//!
//! 会话及其原生 Rime 资源只由引擎工作线程访问；来自客户端和渲染器的事件都先进入
//! 工作队列，再由该线程完成状态变更。引擎据上下文令牌和修订号拒绝过期输入与界面事件。
use crate::client_connection::ClientConnection;
use crate::{
    librime,
    renderer_bridge::{RendererPublisher, render_snapshot},
    session_route,
    worker::Processor,
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use weasel_common::message::{
    ContextToken, Failure, FailureCode, InputOpened, ModeIndicator, ModeIndicatorReason,
};
use weasel_common::{
    message::{
        Envelope, KeyEventResponse, RenderRect, RenderSnapshot, RendererEvent, envelope::Payload,
    },
    process::RuntimePaths,
};

/// 引擎工作队列的有界容量，用于限制待处理事件占用的内存。
pub(crate) const QUEUE_CAPACITY: usize = 128;

/// 一条输入连接多久没打字就回收它的**原生会话**（不是连接、更不是进程）。
///
/// 这是"连接池"的回收侧：省掉的是会话占的词库句柄与组合状态，进程常驻、
/// 连接不断，所以再打字时只需重建会话（毫秒级），不必重新初始化 librime
/// （实测 1010 ms，正是"停一会儿再打第一个字丢失"的根因）。
pub(crate) const SESSION_IDLE_RECLAIM: Duration = Duration::from_secs(300);

/// 等待 TIP 返回模式提示插入点的最长时间。
///
/// 该期限只防止迟到的布局结果重新唤起旧提示；实际显示时长由 Renderer
/// 根据用户配置从收到新提示时开始计算。
const MODE_INDICATOR_LAYOUT_TIMEOUT: Duration = Duration::from_millis(800);

/// 逐键诊断日志是否开启（`WEASEL_ENGINE_VERBOSE=1`）。
///
/// 默认关闭：`诊断:` 那几行每键 3~4 条，`tracing-appender` 是同步无缓冲写盘，
/// 实测每键 1~2ms —— 是"打字卡"里白付的钱。排查"按键没反应/候选不显示"时再打开。
pub(crate) fn verbose_diagnostics() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WEASEL_ENGINE_VERBOSE").is_some())
}

/// 全局配置决定哪些模式变化需要显示短暂提示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputModeIndicatorPolicy {
    /// 获得可编辑焦点及用户主动切换时显示。
    FocusAndSwitch,
    /// 仅在用户主动切换且最终模式确实变化时显示。
    SwitchOnly,
    /// 从不显示。
    Never,
}

impl InputModeIndicatorPolicy {
    /// 从配置快照读取策略；非法值按发布默认值 `switch_only` 回退。
    fn from_settings(settings: Option<&weasel_common::settings::ConfigSnapshot>) -> Self {
        let Some(settings) = settings else {
            return Self::SwitchOnly;
        };
        match settings.required::<String>(".input_mode_indicator") {
            Ok(value) if value == "focus_and_switch" => Self::FocusAndSwitch,
            Ok(value) if value == "switch_only" => Self::SwitchOnly,
            Ok(value) if value == "never" => Self::Never,
            Ok(value) => {
                tracing::error!(
                    value,
                    fallback = "switch_only",
                    "invalid input_mode_indicator; using fallback"
                );
                Self::SwitchOnly
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    fallback = "switch_only",
                    "invalid input_mode_indicator; using fallback"
                );
                Self::SwitchOnly
            }
        }
    }

    /// 判断指定来源是否应创建模式提示。
    fn allows(self, reason: ModeIndicatorReason) -> bool {
        match self {
            Self::FocusAndSwitch => true,
            Self::SwitchOnly => reason == ModeIndicatorReason::UserSwitch,
            Self::Never => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
/// 等待 TIP 返回插入点位置的模式提示请求。
struct PendingModeIndicator {
    /// 与布局回传匹配的不透明标识。
    id: u64,
    /// 已由 Rime 确认的最终模式。
    ascii_mode: bool,
    /// 触发来源，决定主题可采用的外观。
    reason: ModeIndicatorReason,
    /// 服务端接受布局结果的最后时刻。
    expires_at: Instant,
}

/// 工作线程可处理的消息类型。
pub(crate) enum Work {
    /// 客户端请求及其连接存活标记；请求租约在处理完成前阻止连接被回收。
    Message {
        /// 连接的服务端身份。
        client_id: u64,
        /// 承载请求和响应的连接。
        connection: Arc<ClientConnection>,
        /// 连接关闭时变为 `false`，用于丢弃已排队但尚未执行的请求。
        alive: Arc<AtomicBool>,
        /// 解码后的 RPC 信封。
        envelope: Envelope,
        /// 请求处理期间持有的租约；处理后才释放连接回收保护。
        _request: weasel_common::rpc::RequestLease,
    },
    /// 渲染器发来的候选项或其他界面操作。
    Renderer(RendererEvent),
    /// 空闲巡检：回收长时间没打字的**原生会话**（[`Engine::reclaim_idle_sessions`]）。
    ///
    /// 只回收会话，进程、管道连接与 Rime 实例都留着 —— 连接是池，会话是池里按需
    /// 取用的资源。这样既不占着词库句柄，也不会有"冷启动要重新初始化 librime"的
    /// 代价（实测 1010 ms，正是"停顿一会儿再打字第一个字丢失"的根因）。
    ReclaimIdle,
}

/// 某条输入连接在引擎中的原生会话及其路由、渲染快照。
struct ClientSession {
    /// 是否由渲染器显示预编辑文本，而非写入宿主输入框。
    inline_preedit: bool,
    /// 所属客户端连接的身份。
    connection_id: u64,
    /// 响应和布局更新所使用的连接。
    connection: Arc<ClientConnection>,
    /// 连接存活标记；关闭后会话在引擎空闲处理时回收。
    alive: Arc<AtomicBool>,
    /// 仅在引擎工作线程上调用的 Rime 会话；空闲回收后为 `None`，下次按键重建。
    session: Option<librime::RimeSession>,
    /// 最近一次真正处理打字的时间；空闲回收的判据。
    last_typing: Instant,
    /// 会话状态版本；状态变化时递增，用于拒绝过期渲染操作。
    revision: u64,
    /// 绑定输入上下文令牌并跟踪焦点状态的路由。
    route: session_route::SessionRoute,
    /// 最近一次匹配当前输入上下文的宿主光标矩形。
    anchor: RenderRect,
    /// 新组合文本等待宿主布局确认的最低修订号。
    waiting_for_layout: Option<u64>,
    /// 已接受的最新布局修订号，阻止旧几何覆盖新几何。
    latest_layout_revision: Option<u64>,
    /// 等待 TIP 返回插入点位置的短暂模式提示。
    pending_mode_indicator: Option<PendingModeIndicator>,
    /// 用于增量更新及连接能力切换的最近一次输入状态响应。
    last_response: KeyEventResponse,
}

impl ClientSession {
    /// 取可变原生会话；`None` 表示会话已被空闲回收且此刻无法重建。
    ///
    /// 调用点必须先经过 [`Engine::ensure_session`]，因此这里返回 `None` 时
    /// 放弃本次操作即可，绝不 `unwrap`。
    fn session_mut(&mut self) -> Option<&mut librime::RimeSession> {
        self.session.as_mut()
    }
    /// 记一次真实的打字活动，推迟本会话被空闲回收的时间。
    fn touch(&mut self) {
        self.last_typing = Instant::now();
    }
}

/// 运行 Rime 输入会话并协调客户端、宿主布局与渲染器。
///
/// 所有实例状态（包括原生会话）由工作线程独占访问；`clients` 先于 `rime` 声明，
/// 以保证会话在 Rime 引擎析构前销毁。数据目录锁还必须存活到 Rime 的终结回调结束。
pub(crate) struct Engine {
    // None 保留每应用行为；Some 为服务生命周期内的共享模式。
    /// `Some` 表示服务内共享的 ASCII 状态；`None` 表示各应用独立处理。
    global_ascii: Option<bool>,
    /// 是否允许在安全输入框中使用 Rime，由配置快照初始化。
    allow_rime_in_secure_fields: bool,
    /// 哪些输入模式变化应请求主题显示提示。
    input_mode_indicator: InputModeIndicatorPolicy,
    /// 方案信息（用户目录 `schema/schema.toml`）：勾选清单 + 当前方案。
    ///
    /// 用户在这个文件里选了什么，引擎就在新会话上选什么；运行中通过快捷键换了方案，
    /// 也写回这里 —— 这样"方案选择"只存一处，且不会因为重启而丢失。
    schemas: weasel_style::switcher::SchemaTable,
    /// 写回 `schema.toml` 的目标路径（用户目录那份）。
    schema_path: std::path::PathBuf,
    /// 可选配置快照；缺省时使用每应用默认行为。
    settings: Option<weasel_common::settings::ConfigSnapshot>,
    /// 主题决定的"编码画在哪"（`theme.yaml` 的 `style/inline_preedit`）：
    /// false = 组字串画进候选窗（官方出厂默认），true = 画在宿主应用里。
    /// `app_options.<应用>.inline_preedit` 可以逐应用覆盖它。
    theme_inline_preedit: bool,
    // Field order is intentional: destroy every session before dropping the engine.
    /// 以引擎内单调递增的会话标识索引活动输入会话。
    clients: HashMap<u64, ClientSession>,
    /// 原生 Rime 所有者，必须晚于 `clients` 析构。
    rime: librime::Librime,
    // Must outlive the Rime owner, including its finalize callback.
    /// 保持用户数据目录独占，生命周期覆盖 Rime 的初始化和终结。
    _data_lock: crate::data_lock::DataLock,
    /// 当前向渲染器展示的会话标识。
    active_client: Option<u64>,
    /// 向渲染线程发布快照及查询其预编辑文本能力。
    renderer: RendererPublisher,
    /// 下一次分配的会话标识；溢出时拒绝继续分配并触发断言。
    next_session: u64,
    /// 下一次分配的模式提示标识；零保留为“无请求”。
    next_mode_indicator: u64,
}

/// 分配非零且不回绕的模式提示标识。
fn allocate_mode_indicator_id(next: &mut u64) -> u64 {
    let id = *next;
    *next = next
        .checked_add(1)
        .expect("mode indicator identity exhausted");
    id
}

/// 为需要定位的提示建立单槽请求，并把请求标识写入 TIP 响应。
fn request_mode_indicator(
    client: &mut ClientSession,
    response: &mut KeyEventResponse,
    next_id: &mut u64,
    policy: InputModeIndicatorPolicy,
    reason: ModeIndicatorReason,
    ascii_mode: bool,
) {
    if !policy.allows(reason) || response.composing {
        return;
    }
    let id = allocate_mode_indicator_id(next_id);
    client.pending_mode_indicator = Some(PendingModeIndicator {
        id,
        ascii_mode,
        reason,
        expires_at: Instant::now() + MODE_INDICATOR_LAYOUT_TIMEOUT,
    });
    response.mode_indicator_request_id = Some(id);
}

/// 为宿主生成响应副本；外置预编辑时保留提交和组合状态，但清空宿主组合范围。
fn host_response(mut response: KeyEventResponse) -> KeyEventResponse {
    if response.external_preedit {
        // Preserve composing/commit semantics while keeping the host range empty.
        response.composition.clear();
        response.composition_cursor = 0;
    }
    response
}

/// 新组合开始且 Rime 状态确实改变时，使旧光标锚点失效。
///
/// 返回 `true` 表示调用方必须等待本次组合对应的布局，避免用旧位置展示新候选项。
/// 新组合不再清零锚点：沿用上一次的有效锚点，使首键候选窗立即出现（不再等布局），
/// 随后由布局更新把窗口重定位到正确插入点。返回 `false` 表示未触发"等布局"等待。
fn reset_anchor_for_new_composition(
    _previous: &KeyEventResponse,
    _next: &KeyEventResponse,
    _anchor: &mut RenderRect,
) -> bool {
    false
}

/// 判断布局更新能否满足当前等待条件且不会回退已接受的布局版本。
///
/// 未携带修订号的旧版 TIP 保持令牌匹配语义；携带修订号时，两项最低版本约束都必须满足。
fn layout_matches_pending(
    revision: Option<u64>,
    waiting_for_layout: Option<u64>,
    latest_layout_revision: Option<u64>,
) -> bool {
    // Older TIPs omit the revision and retain the token-only behavior. A new
    // TIP must not let an in-flight probe from a previous edit unlock this one.
    revision.is_none_or(|revision| {
        waiting_for_layout.is_none_or(|required| revision >= required)
            && latest_layout_revision.is_none_or(|accepted| revision >= accepted)
    })
}

#[cfg(test)]
mod preedit_tests {
    use super::*;

    #[test]
    fn composition_keeps_last_anchor_and_shows_immediately() {
        let old = KeyEventResponse::default();
        let next = KeyEventResponse {
            composing: true,
            state_updated: true,
            external_preedit: true,
            composition: "nihao".into(),
            ..Default::default()
        };
        // 新组合沿用上一次的有效锚点：首键候选窗立即出现（不再等布局到达），
        // 随后由布局更新把窗口重定位到正确插入点。锚点因此不被清零。
        let mut anchor = RenderRect {
            left: 10,
            right: 11,
            top: 20,
            bottom: 40,
            valid: true,
        };
        assert!(!reset_anchor_for_new_composition(&old, &next, &mut anchor));
        assert!(anchor.valid, "保留上次锚点，避免首键在 (0,0) 闪现");
        assert!(render_snapshot(1, 1, &next, &anchor).visible);
        // 尚无有效锚点时（首键前的极早期），可见性仅由候选/组字状态决定。
        assert!(render_snapshot(1, 1, &next, &RenderRect::default()).visible);
    }

    #[test]
    fn layout_revision_rejects_old_probes_but_accepts_legacy_tip() {
        assert!(!layout_matches_pending(Some(0), Some(5), Some(3)));
        assert!(!layout_matches_pending(Some(4), Some(5), Some(3)));
        assert!(!layout_matches_pending(Some(2), None, Some(3)));
        assert!(layout_matches_pending(Some(5), Some(5), Some(3)));
        assert!(layout_matches_pending(None, Some(5), Some(3)));
    }

    #[test]
    fn external_preedit_keeps_host_composition_and_commit_but_routes_text_to_renderer() {
        let response = KeyEventResponse {
            external_preedit: true,
            composing: true,
            state_updated: true,
            composition: "nihao".into(),
            composition_cursor: 2,
            commit_text: "前文".into(),
            ..Default::default()
        };
        let anchor = RenderRect {
            left: 10,
            right: 10,
            top: 20,
            bottom: 40,
            valid: true,
        };
        let snapshot = render_snapshot(1, 2, &response, &anchor);
        assert!(snapshot.visible); // Input field remains visible without candidates.
        assert_eq!(snapshot.preedit.unwrap().text, "nihao");
        let host = host_response(response.clone());
        assert!(host.composition.is_empty());
        assert_eq!(host.composition_cursor, 0);
        assert!(host.composing && host.state_updated);
        assert_eq!(host.commit_text, "前文");
        let inline = KeyEventResponse {
            external_preedit: false,
            ..response
        };
        assert_eq!(host_response(inline.clone()), inline);
        assert!(render_snapshot(1, 3, &inline, &anchor).preedit.is_none());
    }
}

/// 诊断用：把每次发往 TIP 的响应中携带的提交文本记账到 `%TEMP%\weasel-commit-trace.log`。
///
/// 仅在环境变量 `WEASEL_TRACE_COMMIT` 非空时启用。用于排查"一次提交在宿主文档里出现
/// 两份"这类问题：把服务端**实际发出的**每一条响应（修订号、提交文本字节数、是否仍在
/// 组合）逐条落盘，就能判定重复插入是服务端回了两次，还是客户端把同一响应应用了两次。
fn trace_commit_reply(response: &KeyEventResponse) {
    if std::env::var_os("WEASEL_TRACE_COMMIT").is_none() {
        return;
    }
    let Some(dir) = std::env::var_os("TEMP").or_else(|| std::env::var_os("TMP")) else {
        return;
    };
    let path = std::path::PathBuf::from(dir).join("weasel-commit-trace.log");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    use std::io::Write as _;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let _ = writeln!(
        file,
        "{millis} revision={} commit_bytes={} composing={} candidates={}",
        response.revision,
        response.commit_text.len(),
        response.composing,
        response.candidates.len()
    );
}

/// 键音 / 上屏音（复古打字机音色）。
///
/// 放在服务端播：TIP 位于每个应用进程里（改它要重启应用），渲染器又没有音频接口，
/// 而服务端能看到全部按键与提交，一处即可覆盖所有应用。播放用
/// `PlaySoundW(SND_ASYNC)` **异步**执行，绝不阻塞输入线程。
///
/// 配置直接读运行目录下的 `weasel.yaml`（不经设置界面，改完即生效）：
///
/// ```yaml
/// asset: Rime-Next
/// sound:
///   enabled: true
///   key: sound/typewriter-key.wav
///   commit: sound/typewriter-bell.wav
/// ```
///
/// 宿主配置的格式只有 YAML 一种（2026-10-23 起）：`style::parse_file` 只认 YAML，
/// 配置树与序列化也统一在 `weasel_common::yaml`。旧 `weasel.toml` **一概不读**，
/// 也没有迁移层 —— 读不到就当作没有这份配置。
///
/// 默认关闭；文件缺失或播放失败一律静默。
pub(crate) mod sound {
    /// 最近一次按键的键码，由按键处理路径写入（`process_key`）。
    ///
    /// 服务端的响应里不带键码，所以要给"空格单独一个音"这类分派留一个进程内的记录槽。
    static LAST_KEYCODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

    /// 记录最近一次按键的键码（供按键音分派使用；无需精确同步，按键本身是串行的）。
    pub(crate) fn note_key(keycode: i32) {
        LAST_KEYCODE.store(keycode, std::sync::atomic::Ordering::Relaxed);
    }

    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use std::time::SystemTime;

    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_FILENAME: u32 = 0x0002_0000;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, module: *mut c_void, flags: u32) -> i32;
    }

    #[derive(Default)]
    struct Config {
        enabled: bool,
        key: Option<PathBuf>,
        commit: Option<PathBuf>,
        /// weasel.yaml 的 [sound] mode == "music"：只放歌，不放键音/上屏铃
        music_mode: bool,
        /// 音乐模式要播的文件
        music: Option<PathBuf>,
        /// 用户 weasel.yaml 里显式写了 key 就优先用它，不被 sound\ 的 wav 样本池盖掉
        key_explicit: bool,
    }

    impl Config {
        fn snapshot(&self) -> Self {
            Self {
                enabled: self.enabled,
                key: self.key.clone(),
                key_explicit: self.key_explicit,
                music_mode: self.music_mode,
                music: self.music.clone(),
                commit: self.commit.clone(),
            }
        }
    }

    /// 运行目录：`weasel.yaml` 与数据文件夹所在处。
    fn runtime_dir() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        exe.parent().map(PathBuf::from)
    }

    /// 部署进行中？读用户目录里的 `.deploying` 标记（部署跑在另一个进程里）。
    ///
    /// 按键路径上不能每次去 `stat`，带 500ms 缓存：部署开始/结束最多晚半秒生效，
    /// 对"部署时别响"足够，也不会拖慢打字。
    fn deploying() -> bool {
        static CACHE: OnceLock<Mutex<Option<(std::time::Instant, bool)>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(None));
        let mut guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((at, value)) = *guard
            && at.elapsed() < std::time::Duration::from_millis(500)
        {
            return value;
        }
        let value = weasel_common::process::RuntimePaths::discover()
            .map(|paths| weasel_common::deploy_manifest::is_deploying(&paths.user_data))
            .unwrap_or(false);
        *guard = Some((std::time::Instant::now(), value));
        value
    }

    /// 仅供单元测试：暴露解析入口（生产路径仍然只通过 [`config`] 调用）。
    #[cfg(test)]
    pub(crate) fn parse_for_test(
        text: &str,
        dir: &PathBuf,
    ) -> (bool, Option<PathBuf>, Option<PathBuf>) {
        let config = parse_merged(text, "", Some(dir));
        (config.enabled, config.key.clone(), config.commit.clone())
    }

    /// 仅供单元测试：验证「程序默认 + 用户补丁」的叠加顺序。
    #[cfg(test)]
    pub(crate) fn parse_merged_for_test(
        host: &str,
        user: &str,
    ) -> (bool, Option<PathBuf>, Option<PathBuf>) {
        let config = parse_merged(host, user, None);
        (config.enabled, config.key.clone(), config.commit.clone())
    }

    /// 把**程序文件夹**的 `weasel.yaml` 与**用户目录**的 `weasel.yaml` 叠起来。
    ///
    /// 设置界面的「启用声音」开关写的是用户目录那份（`sound: enabled: false`），
    /// 而以前这里只读程序文件夹的 `weasel.toml` ⇒ 用户在界面上关了，声音照响
    /// （2026-10-18 用户实测报"禁用呢"）。现在按字段叠加：用户那份优先，缺的字段
    /// 沿用程序那份，和 `Server/src/settings.rs` 的叠加顺序一致。
    fn parse_merged(host_text: &str, user_text: &str, dir: Option<&PathBuf>) -> Config {
        #[derive(serde::Deserialize)]
        struct Host {
            #[serde(default)]
            sound: Option<HostSound>,
            #[serde(default)]
            asset: Option<String>,
        }
        #[derive(serde::Deserialize, Default)]
        struct HostSound {
            #[serde(default)]
            enabled: Option<bool>,
            #[serde(default)]
            key: Option<String>,
            #[serde(default)]
            commit: Option<String>,
            #[serde(default)]
            mode: Option<String>,
            #[serde(default)]
            music: Option<String>,
        }

        /// `asset` 指定资源根目录：相对路径从 exe 目录起逐级向上找同名目录
        /// （开发时能找到 <仓库根>\asset；安装后 asset 与 exe 同级，或未配置时用 exe 目录）。
        fn resolve_base(asset: Option<&str>, dir: Option<&PathBuf>) -> Option<PathBuf> {
            let Some(raw) = asset else {
                return dir.cloned();
            };
            let path = PathBuf::from(raw.replace('\\', "/"));
            if path.is_absolute() {
                return Some(path);
            }
            dir.map(|start| {
                let mut here = Some(start.clone());
                while let Some(base) = here {
                    let candidate = base.join(&path);
                    if candidate.is_dir() {
                        return candidate;
                    }
                    here = base.parent().map(PathBuf::from);
                }
                start.join(&path)
            })
        }

        fn resolve_path(raw: Option<&String>, base: &Option<PathBuf>) -> Option<PathBuf> {
            let raw = raw?.trim();
            if raw.is_empty() {
                return None;
            }
            let path = PathBuf::from(raw.replace('\\', "/"));
            if path.is_absolute() {
                return Some(path);
            }
            match base {
                Some(base) => Some(base.join(path)),
                None => Some(path),
            }
        }

        fn load(text: &str) -> Option<Host> {
            if text.trim().is_empty() {
                return None;
            }
            // 只认 YAML：程序文件夹与用户目录的宿主配置都是 `weasel.yaml`。
            // 旧 TOML 一概不读（没有迁移层），读不到就当作没有这份配置。
            weasel_common::yaml::from_str::<Host>(text).ok()
        }

        let host = load(host_text);
        let user = load(user_text);
        let asset = host
            .as_ref()
            .and_then(|host| host.asset.as_deref())
            .or_else(|| user.as_ref().and_then(|user| user.asset.as_deref()));
        let base = resolve_base(asset, dir);
        // 程序那份先铺底，用户那份逐字段覆盖（`enabled = false` 也能盖掉 `true`）。
        let mut sound = HostSound::default();
        for source in [
            host.as_ref().and_then(|host| host.sound.as_ref()),
            user.as_ref().and_then(|user| user.sound.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(enabled) = source.enabled {
                sound.enabled = Some(enabled);
            }
            if source.key.is_some() {
                sound.key = source.key.clone();
            }
            if source.commit.is_some() {
                sound.commit = source.commit.clone();
            }
            if source.mode.is_some() {
                sound.mode = source.mode.clone();
            }
            if source.music.is_some() {
                sound.music = source.music.clone();
            }
        }
        Config {
            enabled: sound.enabled.unwrap_or(false),
            key: prefer_existing(resolve_path(sound.key.as_ref(), &base)),
            commit: prefer_existing(resolve_path(sound.commit.as_ref(), &base)),
            music_mode: sound
                .mode
                .as_deref()
                .map(|mode| mode.eq_ignore_ascii_case("music"))
                .unwrap_or(false),
            music: prefer_existing(resolve_path(sound.music.as_ref(), &base)),
            key_explicit: user
                .as_ref()
                .and_then(|user| user.sound.as_ref())
                .and_then(|sound| sound.key.as_ref())
                .is_some(),
        }
    }

    /// 用户目录的 `sound\`：程序文件夹里找不到同名音频时，用它补位。
    ///
    /// 用户 2026-10-19 要求「加用户文件夹识别」——以前只认 `asset` 指向的
    /// 程序文件夹（`C:\Program Files\Weasel-Next\Rime-Next\sound\`），
    /// 用户把自己的 mp3 放进 `%APPDATA%\Rime-Next\sound\` 会被忽略。
    fn user_sound_dir() -> Option<PathBuf> {
        weasel_common::process::RuntimePaths::discover()
            .ok()
            .map(|paths| paths.user_data.join("sound"))
    }

    /// 把「相对某种基准解析出来的路径」落到磁盘：存在就用它，
    /// 不存在再看用户 `sound\` 里有没有同名文件，最后原样返回（让上层报缺失）。
    fn prefer_existing(path: Option<PathBuf>) -> Option<PathBuf> {
        let candidate = path?;
        if candidate.exists() {
            return Some(candidate);
        }
        if let (Some(dir), Some(name)) = (user_sound_dir(), candidate.file_name()) {
            let alternative = dir.join(name);
            if alternative.exists() {
                return Some(alternative);
            }
        }
        Some(candidate)
    }

    /// 读配置；程序 `weasel.yaml` 或用户 `weasel.yaml` 变化时重建（按 mtime 判断）。
    fn config() -> Config {
        static CACHE: OnceLock<Mutex<(Option<(Option<SystemTime>, Option<SystemTime>)>, Config)>> =
            OnceLock::new();
        let cell = CACHE.get_or_init(|| Mutex::new((None, Config::default())));
        let Some(dir) = runtime_dir() else {
            return Config::default();
        };
        // 宿主配置只有一份：`<程序文件夹>\weasel.yaml`。旧 `weasel.toml` 没有兼容读，
        // 也没有迁移层：没有这份 YAML 就是没有配置，直接用内置默认值。
        let path = Some(weasel_common::settings::program_config_path(&dir))
            .filter(|candidate| candidate.is_file());
        // 用户在设置界面里改的是**用户目录**那份补丁（`weasel.yaml`，2026-10-21 起）。
        let user_path = weasel_common::process::RuntimePaths::discover()
            .ok()
            .map(|paths| weasel_common::settings::user_config_path(&paths.user_data));
        let mtime = |candidate: &Option<PathBuf>| {
            candidate
                .as_ref()
                .and_then(|path| std::fs::metadata(path).and_then(|meta| meta.modified()).ok())
        };
        let stamp = (mtime(&path), mtime(&user_path));
        let Ok(mut cache) = cell.lock() else {
            return Config::default();
        };
        if cache.0 == Some(stamp) {
            return cache.1.snapshot();
        }
        let text = path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        let user_text = user_path
            .as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        let parsed = parse_merged(&text, &user_text, Some(&dir));
        let snapshot = parsed.snapshot();
        cache.0 = Some(stamp);
        cache.1 = parsed;
        snapshot
    }

    /// 多声部播放引擎：多个 `waveOut` 设备 + 内存里的 PCM。
    ///
    /// 现实打字机连打时是**多次击打同时叠加**的，而 `PlaySound` 同时只能播一个声音。
    /// 单个 `waveOut` 设备的缓冲是**串行队列**，所以要重叠必须开多个设备（系统会把它们混音）。
    /// WAV 只在首次使用时读一次、解析进内存，之后每次播放只是一次内存拷贝。
    pub(super) mod voices {
        use std::collections::HashMap;
        use std::ffi::c_void;
        use std::path::PathBuf;
        use std::sync::{Mutex, OnceLock};

        const WAVE_MAPPER: u32 = u32::MAX;
        const CALLBACK_NULL: usize = 0;
        const WAVE_FORMAT_PCM: u16 = 1;
        const WHDR_INQUEUE: u32 = 0x0000_0010;
        /// 同时可叠加的声部数（真人连打很少超过 4~5 声重叠）。
        const VOICE_COUNT: usize = 6;

        #[repr(C)]
        #[derive(Clone, Copy)]
        struct WaveFormatEx {
            format_tag: u16,
            channels: u16,
            samples_per_sec: u32,
            avg_bytes_per_sec: u32,
            block_align: u16,
            bits_per_sample: u16,
            cb_size: u16,
        }

        #[repr(C)]
        struct WaveHdr {
            data: *mut u8,
            buffer_length: u32,
            bytes_recorded: u32,
            user: usize,
            flags: u32,
            loops: u32,
            next: *mut WaveHdr,
            reserved: usize,
        }

        #[link(name = "winmm")]
        unsafe extern "system" {
            fn waveOutOpen(
                handle: *mut *mut c_void,
                device: u32,
                format: *const WaveFormatEx,
                callback: usize,
                instance: usize,
                flags: u32,
            ) -> u32;
            fn waveOutPrepareHeader(handle: *mut c_void, header: *mut WaveHdr, size: u32) -> u32;
            fn waveOutWrite(handle: *mut c_void, header: *mut WaveHdr, size: u32) -> u32;
            fn waveOutUnprepareHeader(handle: *mut c_void, header: *mut WaveHdr, size: u32) -> u32;
            fn waveOutClose(handle: *mut c_void) -> u32;
        }

        struct Voice {
            handle: *mut c_void,
            header: Box<WaveHdr>,
            buffer: Vec<u8>,
        }

        /// 设备句柄与 `WAVEHDR` 只在本模块的互斥锁内访问，因此可以安全跨线程。
        unsafe impl Send for Voice {}

        struct Clip {
            format: WaveFormatEx,
            pcm: Vec<u8>,
        }

        struct Engine {
            voices: Vec<Voice>,
            format: WaveFormatEx,
            clips: HashMap<PathBuf, Clip>,
            next: usize,
        }

        unsafe impl Send for Engine {}

        static ENGINE: OnceLock<Mutex<Option<Engine>>> = OnceLock::new();

        /// 极简 WAV 解析：只要 PCM 与 `fmt `/`data` 两块。
        fn parse_wav(bytes: &[u8]) -> Option<(WaveFormatEx, Vec<u8>)> {
            if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
                return None;
            }
            let mut format = None;
            let mut data = None;
            let mut at = 12usize;
            while at + 8 <= bytes.len() {
                let id = &bytes[at..at + 4];
                let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
                let body = bytes.get(at + 8..at + 8 + size)?;
                if id == b"fmt " && body.len() >= 16 {
                    format = Some(WaveFormatEx {
                        format_tag: u16::from_le_bytes(body[0..2].try_into().ok()?),
                        channels: u16::from_le_bytes(body[2..4].try_into().ok()?),
                        samples_per_sec: u32::from_le_bytes(body[4..8].try_into().ok()?),
                        avg_bytes_per_sec: u32::from_le_bytes(body[8..12].try_into().ok()?),
                        block_align: u16::from_le_bytes(body[12..14].try_into().ok()?),
                        bits_per_sample: u16::from_le_bytes(body[14..16].try_into().ok()?),
                        cb_size: 0,
                    });
                } else if id == b"data" {
                    data = Some(body.to_vec());
                }
                at += 8 + size + (size & 1);
            }
            let format = format?;
            if format.format_tag != WAVE_FORMAT_PCM {
                return None;
            }
            Some((format, data?))
        }

        fn open_voices(format: &WaveFormatEx) -> Vec<Voice> {
            let mut voices = Vec::with_capacity(VOICE_COUNT);
            for _ in 0..VOICE_COUNT {
                let mut handle: *mut c_void = std::ptr::null_mut();
                let code = unsafe {
                    waveOutOpen(
                        &mut handle,
                        WAVE_MAPPER,
                        format,
                        CALLBACK_NULL,
                        0,
                        0,
                    )
                };
                if code != 0 || handle.is_null() {
                    break;
                }
                voices.push(Voice {
                    handle,
                    header: Box::new(WaveHdr {
                        data: std::ptr::null_mut(),
                        buffer_length: 0,
                        bytes_recorded: 0,
                        user: 0,
                        flags: 0,
                        loops: 0,
                        next: std::ptr::null_mut(),
                        reserved: 0,
                    }),
                    buffer: Vec::new(),
                });
            }
            voices
        }

        /// 播放一段 PCM；返回 false 表示本引擎不可用（调用方应退回 `PlaySound`）。
        pub(super) fn play(path: &PathBuf, from_ms: u32, to_ms: u32) -> bool {
            let cell = ENGINE.get_or_init(|| Mutex::new(None));
            let mut guard = match cell.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            // 首次使用：读文件、解析格式、打开多个设备。
            if guard.is_none() {
                let Ok(bytes) = std::fs::read(path) else {
                    return false;
                };
                let Some((format, pcm)) = parse_wav(&bytes) else {
                    return false;
                };
                let voices = open_voices(&format);
                if voices.is_empty() {
                    return false;
                }
                let mut clips = HashMap::new();
                clips.insert(path.clone(), Clip { format, pcm });
                *guard = Some(Engine {
                    voices,
                    format,
                    clips,
                    next: 0,
                });
            }
            let Some(engine) = guard.as_mut() else {
                return false;
            };
            if !engine.clips.contains_key(path) {
                let Ok(bytes) = std::fs::read(path) else {
                    return false;
                };
                let Some((format, pcm)) = parse_wav(&bytes) else {
                    return false;
                };
                // 设备是按第一个片段的格式打开的；格式不同就退回 PlaySound。
                if format.format_tag != engine.format.format_tag
                    || format.channels != engine.format.channels
                    || format.samples_per_sec != engine.format.samples_per_sec
                    || format.bits_per_sample != engine.format.bits_per_sample
                {
                    return false;
                }
                engine.clips.insert(path.clone(), Clip { format, pcm });
            }
            let Some(clip) = engine.clips.get(path) else {
                return false;
            };
            // 按毫秒窗口切片（窗口为 0 时播整段）。
            let align = clip.format.block_align.max(1) as usize;
            let rate = clip.format.avg_bytes_per_sec.max(1) as usize;
            let start = ((from_ms as usize * rate / 1000) / align) * align;
            let end = if to_ms > from_ms {
                (((to_ms as usize * rate / 1000) / align) * align).min(clip.pcm.len())
            } else {
                clip.pcm.len()
            };
            if start >= end || end > clip.pcm.len() {
                return false;
            }
            let slice = clip.pcm[start..end].to_vec();
            // 找一个空闲声部；全都还在播就复用轮转位置（接受切掉最老的一声）。
            let count = engine.voices.len();
            let mut picked = None;
            for offset in 0..count {
                let index = (engine.next + offset) % count;
                let flags = engine.voices[index].header.flags;
                if flags & WHDR_INQUEUE == 0 {
                    picked = Some(index);
                    break;
                }
            }
            let index = picked.unwrap_or_else(|| {
                let index = engine.next % count;
                index
            });
            engine.next = (index + 1) % count;
            let voice = &mut engine.voices[index];
            if voice.header.flags & WHDR_INQUEUE != 0 {
                return false; // 全部声部都在播：这一声放弃，避免破坏正在播放的缓冲。
            }
            if voice.header.flags != 0 {
                unsafe {
                    waveOutUnprepareHeader(
                        voice.handle,
                        voice.header.as_mut(),
                        std::mem::size_of::<WaveHdr>() as u32,
                    )
                };
            }
            voice.buffer.clear();
            voice.buffer.extend_from_slice(&slice);
            voice.header.data = voice.buffer.as_mut_ptr();
            voice.header.buffer_length = voice.buffer.len() as u32;
            voice.header.flags = 0;
            let prepared = unsafe {
                waveOutPrepareHeader(
                    voice.handle,
                    voice.header.as_mut(),
                    std::mem::size_of::<WaveHdr>() as u32,
                )
            };
            if prepared != 0 {
                return false;
            }
            let written = unsafe {
                waveOutWrite(
                    voice.handle,
                    voice.header.as_mut(),
                    std::mem::size_of::<WaveHdr>() as u32,
                )
            };
            written == 0
        }

        /// 进程退出时关闭设备（正常路径用不到，但避免驱动残留）。
        pub(super) fn shutdown() {
            let Some(cell) = ENGINE.get() else {
                return;
            };
            let mut guard = match cell.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(engine) = guard.take() {
                for mut voice in engine.voices {
                    unsafe { waveOutClose(voice.handle) };
                    voice.handle = std::ptr::null_mut();
                }
            }
        }
    }

    /// 播放一个声音文件：mp3 走 MCI（`PlaySound` 不支持 mp3，且 MCI 能给毫秒窗口），
    /// wav 仍走 `PlaySoundW`。
    ///
    /// 两条路径都是**异步**的：立即返回，不给每键增加延迟。`from_ms`/`to_ms` 是播放窗口
    /// （毫秒；`to_ms <= from_ms` 表示整段播放）—— 所以"只播第一击"这类裁剪变成**播放参数**，
    /// 不需要再产出裁剪过的 WAV。
    fn play(path: &PathBuf, from_ms: u32, to_ms: u32) {
        // 诊断：把"要播什么、在不在、走哪条路径"落盘，避免"没声音"再次静默。
        {
            use std::io::Write as _;
            let exists = path.is_file();
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::temp_dir().join("weasel-sound.log"))
            {
                let _ = writeln!(
                    file,
                    "play {:?} exists={} window={}..{}",
                    path, exists, from_ms, to_ms
                );
            }
        }
        use std::os::windows::ffi::OsStrExt as _;
        let is_mp3 = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"));
        if is_mp3 {
            play_mci(path, from_ms, to_ms);
            return;
        }
        // 窗口只对 mp3(MCI) 有意义：WAV 一律整段播放。
        // （否则 1019..1419ms 这种 mp3 时代的窗口会把 650ms 的铃整段跳过，只剩单声部兜底。）
        let (from_ms, to_ms) = (0, 0);
        // wav：优先走多声部引擎（连打时声音可以叠加，像真打字机）；
        // 设备打不开或格式不匹配时退回单声部的 PlaySound，保证永远有声音。
        // 两个声音都走多声部引擎：连打时按键声可叠加，上屏铃响起时也不会掐断余音。
        // 设备打不开或格式不匹配时，下面的 PlaySound 兜底仍然生效。
        if voices::play(path, from_ms, to_ms) {
            return;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let _ = unsafe {
            PlaySoundW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                SND_ASYNC | SND_NODEFAULT | SND_FILENAME,
            )
        };
    }

    /// 背景循环播放一首长音频 —— 用户 2026-10-19：「要能连续播放音乐」。
    ///
    /// 与 `play_mci` 的分工：`play_mci` 是「一次播放」（击键音那种几十毫秒的短音），
    /// 这里是自己持有 alias、自己轮询 `status <alias> mode` 的后台线程：放完立刻从头再来，
    /// 直到进程退出或用户在设置里关掉声音。
    ///
    /// 只在 `weasel.yaml` 显式配了 `key` 且是 mp3 时启用，避免影响原有的 wav 键音。
    fn play_song_loop(path: &PathBuf) {
        static STARTED: OnceLock<()> = OnceLock::new();
        /// 空闲超时后要求下一片回到开头（用户 2026-10-19：1 分钟没打字就重头播）
        static REWIND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let path = path.clone();
        STARTED.get_or_init(|| {
            std::thread::spawn(move || {
                #[link(name = "winmm")]
                unsafe extern "system" {
                    fn mciSendStringW(
                        command: *const u16,
                        buffer: *mut u16,
                        buffer_len: u32,
                        callback: *mut c_void,
                    ) -> u32;
                }
                let send = |command: &str| {
                    let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
                    let _ = unsafe {
                        mciSendStringW(wide.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut())
                    };
                };
                let query = |command: &str| -> String {
                    let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
                    let mut buffer = [0u16; 128];
                    let _ = unsafe {
                        mciSendStringW(
                            wide.as_ptr(),
                            buffer.as_mut_ptr(),
                            buffer.len() as u32,
                            std::ptr::null_mut(),
                        )
                    };
                    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
                    String::from_utf16_lossy(&buffer[..len])
                };
                let file = path.to_string_lossy().to_string();
                let alias = "weaselsong";
                send(&format!("open \"{file}\" type mpegvideo alias {alias}"));
                send(&format!("set {alias} time format milliseconds"));
                loop {
                    // 用户在设置里把声音关掉、或把 key 换成别的文件 → 停止循环
                    let current = config();
                    if !current.enabled || current.key.as_ref() != Some(&path) {
                        return;
                    }
                    send(&format!("play {alias}"));
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(400));
                        if !query(&format!("status {alias} mode"))
                            .trim()
                            .eq_ignore_ascii_case("playing")
                        {
                            break;
                        }
                    }
                }
            });
        });
    }

    /// 用 Windows Media Player 的 COM 控件播放歌曲并循环（用户 2026-10-19）。
    ///
    /// 为什么不用 MCI：本机实测 `mciSendString("open ... type mpegvideo")` 返回 **277
    /// （MCIERR_CANNOT_LOAD_DRIVER）** —— DirectShow 的 MCI 驱动装不上，mp3 根本打不开。
    /// WMP 的 `WMPlayer.OCX` 在本机可用，且 `settings.setMode("loop", true)` 原生支持循环。
    ///
    /// 实现方式：起一个隐藏的 PowerShell 子进程持有播放器（只起一次），
    /// 引擎退出/用户关声音时把它杀掉。这样不用在 Rust 里手写 IDispatch 晚绑定。
    fn play_song(path: &PathBuf) {
        use std::sync::Mutex;
        static CHILD: OnceLock<Mutex<Option<std::process::Child>>> = OnceLock::new();
        let slot = CHILD.get_or_init(|| Mutex::new(None));
        let mut guard = match slot.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // 子进程还活着就说明歌还在播，不重复启动（否则每键起一个新播放器）
        if let Some(child) = guard.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                return;
            }
        }
        // 用 WPF 的 MediaPlayer（System.Windows.Media），不用 WMPlayer.OCX：
        // 本机实测 WMPlayer.OCX 在无窗口的隐藏进程里一直卡在 playState=9(transitioning)，
        // 而 WPF MediaPlayer 能正常出声（2026-10-19 实测位置 0.8→4.9s 递增，时长 177.5s）。
        // 循环靠轮询 Position >= NaturalDuration 后回零重播（无消息泵时 MediaEnded 不触发）。
        // 「打字演奏」模式（用户 2026-10-19 选 C）：每按一键把播放位置往前推一小段，
        // 播 260ms 再暂停 —— 不按键就没有声音，连打时像用键盘弹这首歌。
        //
        // 引擎与脚本之间用「命令文件长度」通信：引擎每收到一键就往
        // %TEMP%\weasel-song-step.txt 追加 1 字节，脚本数长度差决定前进几个 step。
        const SCRIPT: &str = r#"
param([string]$Path)
Add-Type -AssemblyName PresentationCore
$log = Join-Path $env:TEMP 'weasel-song.log'
$cmd = Join-Path $env:TEMP 'weasel-song-step.txt'
$step = 0.25
$player = New-Object System.Windows.Media.MediaPlayer
$player.Open([uri]$Path)
$player.Play()
$player.Pause()
("ready " + $Path) | Set-Content $log
$seen = 0
if (Test-Path $cmd) { $seen = (Get-Item $cmd).Length }
while ($true) {
  Start-Sleep -Milliseconds 50
  if (-not (Test-Path $cmd)) { continue }
  $len = (Get-Item $cmd).Length
  if ($len -le $seen) { $seen = $len; continue }
  $delta = $len - $seen
  $seen = $len
  $dur = $player.NaturalDuration
  if (-not $dur.HasTimeSpan) { continue }
  $pos = $player.Position.TotalSeconds + ($delta * $step)
  if ($pos -ge $dur.TimeSpan.TotalSeconds) { $pos = 0 }
  $player.Position = [TimeSpan]::FromSeconds($pos)
  $player.Play()
  Start-Sleep -Milliseconds 260
  $player.Pause()
  ("{0:HH:mm:ss} pos={1:N2}s delta={2}" -f (Get-Date), $pos, $delta) | Add-Content $log
}
"#;
        let script = std::env::temp_dir().join("weasel-song.ps1");
        if std::fs::write(&script, SCRIPT).is_err() {
            return;
        }
        let spawned = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
            ])
            .arg(&script)
            .arg("-Path")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        {
            use std::io::Write as _;
            if let Ok(mut handle) = std::fs::OpenOptions::new().create(true).append(true)
                .open(std::env::temp_dir().join("weasel-song-trace.log"))
            {
                let _ = writeln!(handle, "spawn ok={} path={:?}", spawned.is_ok(), path);
            }
        }
        if let Ok(child) = spawned {
            *guard = Some(child);
        }
    }

    /// 打字演奏：往命令文件追加 1 字节，脚本据此把播放位置往前推一步。
    fn step_song() {
        use std::io::Write as _;
        if let Ok(mut handle) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("weasel-song-step.txt"))
        {
            let _ = handle.write_all(b"x");
        }
    }

    /// 关声音时把后台播放器杀掉。
    fn stop_song() {
        use std::sync::Mutex;
        static CHILD: OnceLock<Mutex<Option<std::process::Child>>> = OnceLock::new();
        // 与 `play_song` 共用同一个槽位做不到（静态变量按函数隔离），
        // 这里改用进程级清理：按命令行匹配杀掉 weasel-song.ps1。
        let _ = CHILD;
        let _ = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-WindowStyle",
                "Hidden",
                "-Command",
                "Get-CimInstance Win32_Process -Filter \"Name='powershell.exe'\" | Where-Object { $_.CommandLine -like '*weasel-song.ps1*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    /// 用 MCI 播放 mp3。
    ///
    /// 别名按调用序号轮换（4 个），这样上一声不会被这一声立刻掐断；`close` 之后重新
    /// `open` 是为了让每次播放都从头开始。所有命令都是异步的，失败也只是不出声。
    fn play_mci(path: &PathBuf, from_ms: u32, to_ms: u32) {
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn mciSendStringW(
                command: *const u16,
                buffer: *mut u16,
                buffer_len: u32,
                callback: *mut c_void,
            ) -> u32;
        }
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        /// 已打开的文件 → 别名。`open` 一个 mp3 要重建 DirectShow 滤镜图（几十毫秒），
        /// 每键都 open/close 会在快速连打时排队、甚至彻底没声；打开后只做 stop/play 就很快。
        static OPEN: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
        let send = |command: &str| {
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = unsafe {
                mciSendStringW(wide.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut())
            };
        };
        // 查 MCI 状态的辅助闭包（用于判断"歌还在播"）
        let query = |command: &str| -> String {
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            let mut buffer = [0u16; 128];
            let _ = unsafe {
                mciSendStringW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32, std::ptr::null_mut())
            };
            let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            String::from_utf16_lossy(&buffer[..len])
        };
        let file = path.to_string_lossy().to_string();
        let alias = {
            let mut open = match OPEN.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            match open.iter().find(|(opened, _)| opened == path) {
                Some((_, alias)) => alias.clone(),
                None => {
                    let alias = format!(
                        "weaselsnd{}",
                        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    );
                    send(&format!("open \"{file}\" type mpegvideo alias {alias}"));
                    // 必须声明时间单位为毫秒，否则 `from/to` 会按设备默认单位解释，
                    // 窗口就落到别处（表现为"上屏没声音"或声音不对）。
                    send(&format!("set {alias} time format milliseconds"));
                    open.push((path.clone(), alias.clone()));
                    alias
                }
            }
        };
        // 用户 2026-10-19：整曲（歌曲）已经在播就别重开，
        // 否则每按一键都从 0 秒重来，只有开头那一瞬；播完了下一键自然重来。
        if to_ms <= from_ms {
            let mode = query(&format!("status {alias} mode"));
            if mode.trim().eq_ignore_ascii_case("playing") {
                return;
            }
        }
        send(&format!("stop {alias}"));
        if to_ms > from_ms {
            send(&format!("play {alias} from {from_ms} to {to_ms}"));
        } else {
            send(&format!("play {alias}"));
        }
    }

    /// 上屏铃的播放窗口：`bell.mp3` 里第一击在 1019 ms、第二击在 1119 ms，
    /// 所以 1019→1419 ms 正好是那声"叮-铃"（原始录音不动，窗口只在播放时生效）。
    const BELL_FROM_MS: u32 = 1019;
    const BELL_TO_MS: u32 = 1419;

    /// 击键音样本池：`sounds/typewriter-key-*.wav`（首次调用时枚举一次）。
    ///
    /// 真机的每一次击键都不同（商业音效库也是从样本池随机取），固定播同一个文件会有
    /// 明显的"电子循环感"。
    fn key_pool(dir: &PathBuf) -> &'static [PathBuf] {
        static POOL: OnceLock<Vec<PathBuf>> = OnceLock::new();
        POOL.get_or_init(|| {
            let Ok(entries) = std::fs::read_dir(dir.join("sound")) else {
                return Vec::new();
            };
            let mut pool: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| {
                            name.starts_with("typewriter-key-") && name.ends_with(".wav")
                        })
                })
                .collect();
            pool.sort();
            pool
        })
        .as_slice()
    }

    /// 从样本池里随机取一个击键音；池为空时退回配置中的 `key`。
    ///
    /// 用系统时间的纳秒做种、配一个原子计数推进，因此不需要引入随机数依赖，
    /// 连续两次也不会取到同一个文件。
    fn pick_key(config: &Config, dir: &PathBuf) -> Option<PathBuf> {
        // 用户显式配了 key（mp3 之类）→ 直接用它，不被 wav 样本池随机盖掉。
        if config.key_explicit {
            return config.key.clone();
        }
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let pool = key_pool(dir);
        if pool.is_empty() {
            return config.key.clone();
        }
        let mixed = COUNTER
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .wrapping_mul(6364136223846793005)
            .wrapping_add(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|elapsed| u64::from(elapsed.subsec_nanos()))
                    .unwrap_or(0),
            );
        Some(pool[(mixed >> 33) as usize % pool.len()].clone())
    }

    /// 「打字演奏」（用户 2026-10-19 定案）：**不起命令行、不跳段**，按键累计后依次播放。
    ///
    /// 设计要点：
    /// * mp3 在**引擎进程内**用纯 Rust 的 `nanomp3` 解码成 PCM，不 spawn powershell（没有任何命令行窗口）；
    /// * 每次按键只做一次原子自增（微秒级），不给打字加延迟；
    /// * 后台线程按队列逐片播放：取 `slice_ms` 长的一片 → 写成临时 wav → 交给现有
    ///   `voices`（waveOut 多声部）→ **等这一片放完**再取下一条，所以永远不会压在上一段上；
    /// * 队列为空就静音，不按键不出声；
    /// * 配置写在 `%APPDATA%\Rime-Next\sound\sound.yaml` 的 `typing_song` 里。
    pub(super) mod typing_song {
        use std::io::Write as _;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Mutex, OnceLock};
        use std::time::Duration;

        #[derive(serde::Deserialize, Default, Clone)]
        pub struct Settings {
            pub enabled: Option<bool>,
            pub file: Option<String>,
            pub slice_ms: Option<u32>,
        }

        #[derive(serde::Deserialize, Default)]
        struct Root {
            typing_song: Option<Settings>,
            mode: Option<Mode>,
        }

        /// 声音模式：music = 打字演奏（放歌），effect = 打字机音效。
        #[derive(serde::Deserialize, Default, Clone)]
        pub struct Mode {
            pub kind: Option<String>,
        }

        struct Player {
            /// 这首歌的来源文件（换歌时要重开设备、重新解码）
            path: std::path::PathBuf,
            samples: Vec<i16>,
            rate: u32,
            channels: u16,
            cursor: usize,
            slice: usize,
        }

        static QUEUE: AtomicUsize = AtomicUsize::new(0);
        /// 空闲超时后要求下一片回到开头（用户 2026-10-19：1 分钟没打字就重头播）
        static REWIND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        static PLAYER: OnceLock<Mutex<Option<Player>>> = OnceLock::new();
        static STARTED: OnceLock<()> = OnceLock::new();
        static SETTINGS: OnceLock<Mutex<Option<(Option<std::time::SystemTime>, Option<(std::path::PathBuf, u32)>)>>> =
            OnceLock::new();

        /// 读 `%APPDATA%\Rime-Next\sound\sound.yaml`（按 mtime 缓存，避免每键读盘）。
        /// 声音配置文件的路径：**优先 `用户文件夹\sound\sound.yaml`**（和音频放一起），
        /// 旧版放在用户目录根部的 `sound.yaml` 仍然认，作为兼容。
        /// 2026-10-27：TOML 退出，只认 YAML（与全仓库一致）。
        fn config_path(user_data: &std::path::Path) -> Option<std::path::PathBuf> {
            let inside = user_data.join("sound").join("sound.yaml");
            if inside.is_file() {
                return Some(inside);
            }
            let legacy = user_data.join("sound.yaml");
            if legacy.is_file() {
                return Some(legacy);
            }
            None
        }

        pub fn settings() -> Option<(std::path::PathBuf, u32)> {
            // 音乐模式且 [sound] music 指向存在的文件 → 直接用它
            let sound = super::config();
            if sound.music_mode {
                if let Some(path) = sound.music.clone() {
                    if path.is_file() {
                        // 每键推进的时长。用户口径：250 ms 太短（"按一下就没了"）→ 先要 1000 ms，
                        // 又改回 500 ms（2026-10-23）。500 ms/键 ⇒ 每秒 2 键以上就连续播歌。
                        return Some((path, 500));
                    }
                }
            }
            let dir = weasel_common::process::RuntimePaths::discover().ok()?.user_data;
            let Some(path) = config_path(&dir) else { return None; };
            let mtime = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();
            let cell = SETTINGS.get_or_init(|| Mutex::new(None));
            let mut cache = match cell.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some((stamp, value)) = cache.as_ref() {
                if *stamp == mtime {
                    return value.clone();
                }
            }
            let value = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| weasel_common::yaml::from_str::<Root>(&text).ok())
                .and_then(|root| root.typing_song)
                .and_then(|settings| {
                    if !settings.enabled.unwrap_or(false) {
                        return None;
                    }
                    let raw = settings.file?;
                    let mut file = std::path::PathBuf::from(raw.replace('\\', "/"));
                    if !file.is_absolute() {
                        file = dir.join("sound").join(file.file_name()?);
                    }
                    if !file.is_file() {
                        return None;
                    }
                    Some((file, settings.slice_ms.unwrap_or(250)))
                });
            *cache = Some((mtime, value.clone()));
            value
        }

        /// 声音模式（用户 2026-10-19）：`sound.yaml` 的 `mode: kind`
        ///
        /// * `"music"` = 打字演奏：只放歌，**不放**打字机键音与上屏铃声
        /// * `"effect"` = 传统音效：键音 + 上屏铃（默认）
        pub fn music_mode() -> bool {
            // weasel.yaml 的 [sound] mode == "music" 优先（设置界面里那个下拉框）
            if super::config().music_mode {
                return true;
            }
            static CACHE: OnceLock<Mutex<(Option<std::time::SystemTime>, bool)>> = OnceLock::new();
            let Some(dir) = weasel_common::process::RuntimePaths::discover().ok().map(|p| p.user_data) else { return false; };
            let Some(path) = config_path(&dir) else { return false; };
            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            let cell = CACHE.get_or_init(|| Mutex::new((None, false)));
            let mut cache = match cell.lock() { Ok(g) => g, Err(p) => p.into_inner() };
            if cache.0 == mtime { return cache.1; }
            let value = std::fs::read_to_string(&path).ok()
                .and_then(|text| weasel_common::yaml::from_str::<Root>(&text).ok())
                .map(|root| {
                    if let Some(mode) = root.mode {
                        return mode.kind.as_deref().map(|k| k.eq_ignore_ascii_case("music")).unwrap_or(false);
                    }
                    // 没写 [mode] 时按旧行为：有 [typing_song] 且 enabled 就算音乐模式
                    root.typing_song.map(|s| s.enabled.unwrap_or(false)).unwrap_or(false)
                })
                .unwrap_or(false);
            *cache = (mtime, value);
            value
        }

        /// 直接用 `PlaySoundW` 播一个 wav（异步、不阻塞）。
        ///
        /// 为什么绕开 `voices`：2026-10-19 实测本机 `voices::play` 返回 true 却听不到
        /// 任何声音（多声部 waveOut 设备打不开但没报错），而同一个 wav 用 PlaySound 正常。
        fn playsound(path: &std::path::Path) -> bool {
            use std::os::windows::ffi::OsStrExt as _;
            #[link(name = "winmm")]
            unsafe extern "system" {
                fn PlaySoundW(
                    sound: *const u16,
                    module: *mut std::ffi::c_void,
                    flags: u32,
                ) -> i32;
            }
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            // SND_FILENAME | SND_ASYNC | SND_NODEFAULT
            let ok = unsafe { PlaySoundW(wide.as_ptr(), std::ptr::null_mut(), 0x0002_0000 | 0x0000_0001 | 0x0000_0002) };
            ok != 0
        }

        /// 最近一次按键的时间（`step` 写、播放线程读，用来判断"停手了没有"）。
        fn last_key_cell() -> &'static Mutex<Option<std::time::Instant>> {
            static LAST: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
            LAST.get_or_init(|| Mutex::new(None))
        }

        /// 距离上次按键过了多久（还没按过则 `None`）。
        fn idle_for() -> Option<Duration> {
            let cell = last_key_cell();
            let guard = match cell.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.map(|instant| instant.elapsed())
        }

        /// 记一次按键（累计，不阻塞）。
        pub fn step() {
            // 用户 2026-10-19：空闲超过 1 分钟 → 下次按键从歌的开头重新开始。
            let cell = last_key_cell();
            let mut last = match cell.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Some(previous) = *last {
                if previous.elapsed() >= Duration::from_secs(60) {
                    REWIND.store(true, Ordering::Relaxed);
                }
            }
            *last = Some(std::time::Instant::now());
            QUEUE.fetch_add(1, Ordering::Relaxed);
        }

        /// 确保后台播放线程已启动（幂等）。
        pub fn start() {
            STARTED.get_or_init(|| {
                std::thread::spawn(|| loop {
                    if QUEUE.load(Ordering::Relaxed) == 0 {
                        // 队列空了并且已经停手 600 ms 以上：用**歌里真实内容**做一段淡出。
                        // 不补的话最后一帧被硬切（"啪"）；补一个合成常数就是"噗"（都踩过了）。
                        if idle_for().is_none_or(|idle| idle >= Duration::from_millis(600)) {
                            settle_tail();
                        }
                        std::thread::sleep(Duration::from_millis(25));
                        continue;
                    }
                    QUEUE.fetch_sub(1, Ordering::Relaxed);
                    play_one_slice();
                });
            });
        }

        fn play_one_slice() {
            // 诊断：音乐模式没声音时，靠这段判断卡在哪一步
            let trace = |msg: &str| {
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true)
                    .open(std::env::temp_dir().join("weasel-typing.log"))
                {
                    let _ = writeln!(f, "{msg}");
                }
            };
            trace(&format!("slice called music_mode={}", music_mode()));
            let Some((file, slice_ms)) = settings() else {
                trace("settings = None（没有可用的音乐配置）");
                return;
            };
            let slot = PLAYER.get_or_init(|| Mutex::new(None));
            let mut guard = match slot.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            // 第一次、或换了歌 → 冲掉旧设备队列再重新解码。
            if guard.as_ref().map_or(true, |player| player.path != file) {
                stream::reset();
                *guard = load(&file, slice_ms);
            }
            let Some(player) = guard.as_mut() else {
                return;
            };
            if player.samples.is_empty() {
                return;
            }
            if REWIND.swap(false, Ordering::Relaxed) {
                player.cursor = 0;
                // 回到开头：旧队列里还排着上一轮的尾巴，先冲掉，别串音。
                stream::reset();
            }
            let total = player.samples.len();
            let len = player.slice.min(total);
            let start = player.cursor.min(total.saturating_sub(1));
            let mut chunk = Vec::with_capacity(len);
            let mut index = start;
            for _ in 0..len {
                chunk.push(player.samples[index]);
                index += 1;
                if index >= total {
                    index = 0;
                }
            }
            // 无缝推流：切片连着的，写进同一个 waveOut 设备的**串行队列**就没有接缝。
            // 只有"停手/复播"这一次边界需要手动淡出淡入（见 settle_tail）。
            match stream::push(&chunk, player.rate, player.channels) {
                Some((true, depth)) => {
                    // 只有真排进队列才推进播放位置。
                    player.cursor = index;
                    trace(&format!("stream push start={} len={} depth={}", start, len, depth));
                }
                Some((false, depth)) => {
                    // 队列满 → 这一片没播。**cursor 绝对不能推进**，否则就是"歌偷偷跳过去了"。
                    trace(&format!("stream drop start={} depth={}", start, depth));
                }
                None => {
                    // 设备打不开（极少见）→ 退回老的切片播放：有爆音，但至少有声音。
                    trace("waveOut 不可用，退回 PlaySound 切片");
                    fallback_slice(&chunk, player.rate, player.channels, slice_ms);
                    player.cursor = index;
                }
            }
        }

        /// 停手：从**歌里接下来的 40 ms** 取一段，按衰减包络推进设备。
        ///
        /// 关键：不能再用"最后一个采样值"合成常数衰减 —— 那是直流，听感就是一个
        /// 40 ms 的低频"噗"叠在歌底下（2026-10-22 用户报"两段同时响、叠着"）。
        /// 也**不推进 cursor**：复播从同一位置接上，这 40 ms 会重复一次但已衰减到 0，
        /// 听不出来；反过来推进 cursor 会让每次停顿都吃掉歌里一小段。
        fn settle_tail() {
            if !stream::wants_tail() {
                return;
            }
            let slot = PLAYER.get_or_init(|| Mutex::new(None));
            let guard = match slot.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let Some(player) = guard.as_ref() else {
                return;
            };
            let total = player.samples.len();
            if total == 0 {
                return;
            }
            let channels = usize::from(player.channels.max(1));
            let frames = (player.rate / 1000 * stream::edge_ms()).max(1) as usize;
            let mut tail = Vec::with_capacity(frames * channels);
            let mut index = player.cursor % total;
            for frame in 0..frames {
                let gain = 1.0 - frame as f32 / frames as f32;
                for _ in 0..channels {
                    tail.push((player.samples[index] as f32 * gain) as i16);
                    index += 1;
                    if index >= total {
                        index = 0;
                    }
                }
            }
            stream::settled_tail(&tail, player.rate, player.channels);
        }

        /// 兜底：老的"临时 wav + PlaySound"切片播放。
        ///
        /// 只在 waveOut 打不开时用。每片两端淡入淡出、播完 sleep 一片的时间 ——
        /// 有爆音和 4 Hz 泵动（这就是 2026-10-19 那一版的做法），但不会没声音。
        fn fallback_slice(chunk: &[i16], rate: u32, channels: u16, slice_ms: u32) {
            let mut chunk = chunk.to_vec();
            let fade = ((rate as usize / 1000) * 8).max(1) * channels as usize;
            let half = chunk.len() / 2;
            let fade = fade.min(half);
            for index in 0..fade {
                let gain = index as f32 / fade as f32;
                let front = index;
                let back = chunk.len() - 1 - index;
                chunk[front] = (chunk[front] as f32 * gain) as i16;
                chunk[back] = (chunk[back] as f32 * gain) as i16;
            }
            let wav = wav_bytes(&chunk, rate, channels);
            let temp = std::env::temp_dir().join("weasel-typing-note.wav");
            if std::fs::write(&temp, &wav).is_err() {
                return;
            }
            playsound(&temp);
            std::thread::sleep(Duration::from_millis(u64::from(slice_ms)));
        }

        /// 音乐模式的**无缝推流**播放器（2026-10-21 重做）。
        ///
        /// 为什么弃用「临时 wav + PlaySound」切片（那是 2026-10-19 的做法）：
        /// `PlaySound` 是**单通道、播新的就掐掉旧的**，而每片只有 250 ms、写完只 sleep 250 ms，
        /// 扣掉写文件与启动的耗时，下一片必定在上一片淡出还没走完时把它打断
        /// ⇒ 每 250 ms 一个爆音；再加上每片两端各 8 ms 淡入淡出 ⇒ 4 Hz 泵动。
        /// 用户听感就是「很炸、很难听」。
        ///
        /// `waveOut` 的缓冲是**串行队列**：连着 Write 几段，放出来就是连续的。
        /// 所以片与片之间不再需要任何淡入淡出 —— 只有「停手」和「复播」这两个边界需要。
        mod stream {
            use std::ffi::c_void;
            use std::sync::{Mutex, OnceLock};

            const WAVE_MAPPER: u32 = u32::MAX;
            const CALLBACK_NULL: usize = 0;
            const WHDR_DONE: u32 = 0x0000_0001;
            const WHDR_PREPARED: u32 = 0x0000_0002;

            /// 最多排几片（每片 250 ms）→ 约 500 ms 前瞻。
            /// 必须设上限：不设的话打字快过每秒 4 键时队列无限增长，
            /// 几十秒后播的就是很久以前切下的内容（「越打越拖」也是难听的一部分）。
            /// 也别设太大：3 片（750 ms）时手一停音乐还会自己往下走一大段。
            const MAX_PENDING: usize = 2;
            /// 整体音量。旧注释写「整体降到 85%」，实际只缩了淡入淡出那几个采样，
            /// 中段仍是满幅 —— 这首 mp3 本身压得很死，满幅播出来就是「炸」。
            const GAIN: f32 = 0.55;
            /// 停手淡出 / 复播淡入的长度（毫秒）。
            const EDGE_MS: u32 = 40;

            #[repr(C)]
            #[derive(Clone, Copy, PartialEq, Eq)]
            struct Format {
                tag: u16,
                channels: u16,
                rate: u32,
                byte_rate: u32,
                block_align: u16,
                bits: u16,
                cb_size: u16,
            }

            #[repr(C)]
            struct Header {
                data: *mut u8,
                length: u32,
                recorded: u32,
                user: usize,
                flags: u32,
                loops: u32,
                next: *mut Header,
                reserved: usize,
            }

            #[link(name = "winmm")]
            unsafe extern "system" {
                fn waveOutOpen(
                    handle: *mut *mut c_void,
                    device: u32,
                    format: *const Format,
                    callback: usize,
                    instance: usize,
                    flags: u32,
                ) -> u32;
                fn waveOutPrepareHeader(handle: *mut c_void, header: *mut Header, size: u32) -> u32;
                fn waveOutWrite(handle: *mut c_void, header: *mut Header, size: u32) -> u32;
                fn waveOutUnprepareHeader(
                    handle: *mut c_void,
                    header: *mut Header,
                    size: u32,
                ) -> u32;
                fn waveOutReset(handle: *mut c_void) -> u32;
                fn waveOutClose(handle: *mut c_void) -> u32;
            }

            struct Pending {
                header: Box<Header>,
                /// PCM 字节。**堆地址必须稳定**：`header.data` 指向它，
                /// 所以这个 Vec 放进队列后绝不能再扩容。
                data: Vec<u8>,
            }

            /// `Header` 里只有裸指针；整块只在本模块的互斥锁内使用。
            unsafe impl Send for Pending {}

            struct Device {
                handle: *mut c_void,
                format: Format,
                pending: Vec<Pending>,
                /// 最后推出去的那个采样值：停手时用它合成淡出尾巴
                last_sample: i16,
                /// 已经补过淡出尾巴（等下一片要淡入）
                settled: bool,
                needs_fade_in: bool,
            }

            unsafe impl Send for Device {}

            static DEVICE: OnceLock<Mutex<Option<Device>>> = OnceLock::new();

            fn cell() -> &'static Mutex<Option<Device>> {
                DEVICE.get_or_init(|| Mutex::new(None))
            }

            fn lock() -> std::sync::MutexGuard<'static, Option<Device>> {
                match cell().lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                }
            }

            /// 释放一个已经不再被驱动使用的缓冲。
            unsafe fn release(handle: *mut c_void, mut pending: Pending) {
                unsafe {
                    if pending.header.flags & WHDR_PREPARED != 0 {
                        waveOutUnprepareHeader(
                            handle,
                            &mut *pending.header,
                            std::mem::size_of::<Header>() as u32,
                        );
                    }
                }
            }

            /// 回收已经播完的缓冲，返回剩余排队数。
            fn reclaim(device: &mut Device) -> usize {
                let handle = device.handle;
                let mut kept = Vec::with_capacity(device.pending.len());
                for pending in device.pending.drain(..) {
                    if pending.header.flags & WHDR_DONE != 0 {
                        unsafe { release(handle, pending) };
                    } else {
                        kept.push(pending);
                    }
                }
                device.pending = kept;
                device.pending.len()
            }

            /// 把一段 PCM 写进设备的串行队列；调用方已保证队列有空位。
            fn enqueue(device: &mut Device, samples: &[i16]) -> bool {
                if samples.is_empty() {
                    return false;
                }
                let mut data = Vec::with_capacity(samples.len() * 2);
                for sample in samples {
                    let scaled = (*sample as f32 * GAIN)
                        .clamp(f32::from(i16::MIN), f32::from(i16::MAX))
                        as i16;
                    data.extend_from_slice(&scaled.to_le_bytes());
                }
                let mut header = Box::new(Header {
                    data: data.as_mut_ptr(),
                    length: data.len() as u32,
                    recorded: 0,
                    user: 0,
                    flags: 0,
                    loops: 0,
                    next: std::ptr::null_mut(),
                    reserved: 0,
                });
                let size = std::mem::size_of::<Header>() as u32;
                let pointer = &mut *header as *mut Header;
                unsafe {
                    if waveOutPrepareHeader(device.handle, pointer, size) != 0 {
                        return false;
                    }
                    header.flags |= WHDR_PREPARED;
                    if waveOutWrite(device.handle, pointer, size) != 0 {
                        waveOutUnprepareHeader(device.handle, pointer, size);
                        return false;
                    }
                }
                device.last_sample = *samples.last().unwrap_or(&0);
                device.settled = false;
                // data 的堆缓冲在 move 进队列后地址不变，header.data 仍然有效。
                device.pending.push(Pending { header, data });
                true
            }

            /// 关掉设备、丢掉所有排队缓冲（换歌 / 回到开头时用）。
            pub fn reset() {
                let mut guard = lock();
                let Some(mut device) = guard.take() else {
                    return;
                };
                unsafe {
                    waveOutReset(device.handle);
                    for pending in device.pending.drain(..) {
                        release(device.handle, pending);
                    }
                    waveOutClose(device.handle);
                }
            }

            /// 推一片进去。
            ///
            /// 返回 `None` = 设备不可用（调用方应退回 PlaySound）；
            /// `Some((true, 深度))` = 已经排进队列；`Some((false, 深度))` = 队列满、**这一片没播**。
            ///
            /// ⚠️ 调用方必须靠这个返回值决定要不要推进播放位置。早先不管有没有排进去都推进，
            /// 结果按住一个键（系统自动重复 ≈30 次/秒）时歌的位置按 7.5 秒/秒往前跳，
            /// 一松手就从很后面接上 —— 用户 2026-10-22 报的「一直按着就直接跳到后面」。
            fn submit(
                samples: &[i16],
                rate: u32,
                channels: u16,
                tail: bool,
            ) -> Option<(bool, usize)> {
                let channels = channels.max(1);
                let rate = rate.max(8000);
                let wanted = Format {
                    tag: 1,
                    channels,
                    rate,
                    byte_rate: rate * u32::from(channels) * 2,
                    block_align: channels * 2,
                    bits: 16,
                    cb_size: 0,
                };
                let mut guard = lock();
                let need_open = guard.as_ref().map_or(true, |device| device.format != wanted);
                if need_open {
                    if let Some(mut old) = guard.take() {
                        unsafe {
                            waveOutReset(old.handle);
                            for pending in old.pending.drain(..) {
                                release(old.handle, pending);
                            }
                            waveOutClose(old.handle);
                        }
                    }
                    let mut handle: *mut c_void = std::ptr::null_mut();
                    let code = unsafe {
                        waveOutOpen(&mut handle, WAVE_MAPPER, &wanted, CALLBACK_NULL, 0, 0)
                    };
                    if code != 0 || handle.is_null() {
                        return None;
                    }
                    *guard = Some(Device {
                        handle,
                        format: wanted,
                        pending: Vec::new(),
                        last_sample: 0,
                        settled: false,
                        needs_fade_in: true,
                    });
                }
                let Some(device) = guard.as_mut() else {
                    return None;
                };
                let depth = reclaim(device);
                if depth >= MAX_PENDING {
                    // 打字比播放快：这一片直接丢掉，保证实时，别越拖越远。
                    return Some((false, depth));
                }
                let mut slice = samples.to_vec();
                // 只有"复播的第一片"要淡入；淡出尾巴自己已经带了包络，不再套。
                if !tail && device.needs_fade_in {
                    let frames = (device.format.rate / 1000 * EDGE_MS).max(1) as usize;
                    let edge = (frames * usize::from(device.format.channels)).min(slice.len());
                    if edge > 0 {
                        for index in 0..edge {
                            let gain = index as f32 / edge as f32;
                            slice[index] = (slice[index] as f32 * gain) as i16;
                        }
                    }
                    device.needs_fade_in = false;
                }
                if !enqueue(device, &slice) {
                    return None;
                }
                if tail {
                    device.settled = true;
                    device.needs_fade_in = true;
                }
                Some((true, device.pending.len()))
            }

            pub fn push(
                samples: &[i16],
                rate: u32,
                channels: u16,
            ) -> Option<(bool, usize)> {
                submit(samples, rate, channels, false)
            }

            /// 停手淡出：`samples` 必须是**歌里真实的下一段**（调用方按包络衰减好）。
            ///
            /// 早先这里用"最后一个采样值"合成常数衰减 —— 那是直流，听着就是一个
            /// 40 ms 的低频"噗"叠在歌底下（用户 2026-10-22 报的"两段同时响、叠着"）。
            pub fn settled_tail(
                samples: &[i16],
                rate: u32,
                channels: u16,
            ) -> Option<(bool, usize)> {
                submit(samples, rate, channels, true)
            }

            /// 淡入/淡出的长度（毫秒），给调用方切尾巴用。
            pub fn edge_ms() -> u32 {
                EDGE_MS
            }

            /// 是否还需要补一条淡出尾巴（有排队音频、且还没补过）。
            pub fn wants_tail() -> bool {
                let mut guard = lock();
                let Some(device) = guard.as_mut() else {
                    return false;
                };
                if device.settled {
                    return false;
                }
                reclaim(device) > 0
            }
        }

        fn load(path: &std::path::Path, slice_ms: u32) -> Option<Player> {
            let bytes = std::fs::read(path).ok()?;
            // 纯 Rust 解码（nanomp3）：一次性解出整首 PCM，后续切片不再碰文件
            let decoded = nanomp3::decode_all::<i16>(&bytes);
            let rate = decoded.sample_rate;
            let channels = decoded.channels.map(|value| value.num() as u16).unwrap_or(2);
            let samples = decoded.samples;
            if samples.is_empty() || rate == 0 {
                return None;
            }
            let per_ms = (rate as usize / 1000).max(1);
            let slice = (per_ms * slice_ms as usize).max(1) * channels as usize;
            Some(Player {
                path: path.to_path_buf(),
                samples,
                rate,
                channels,
                cursor: 0,
                slice,
            })
        }

        fn wav_bytes(samples: &[i16], rate: u32, channels: u16) -> Vec<u8> {
            let data_len = (samples.len() * 2) as u32;
            let mut out = Vec::with_capacity(44 + data_len as usize);
            out.extend_from_slice(b"RIFF");
            out.extend_from_slice(&(36 + data_len).to_le_bytes());
            out.extend_from_slice(b"WAVEfmt ");
            out.extend_from_slice(&16u32.to_le_bytes());
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&channels.to_le_bytes());
            out.extend_from_slice(&rate.to_le_bytes());
            out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
            out.extend_from_slice(&(channels * 2).to_le_bytes());
            out.extend_from_slice(&16u16.to_le_bytes());
            out.extend_from_slice(b"data");
            out.extend_from_slice(&data_len.to_le_bytes());
            for sample in samples {
                out.extend_from_slice(&sample.to_le_bytes());
            }
            out
        }

        /// 诊断：把"这次播放从哪开始、多长"写到临时文件，方便核对。
        pub fn trace(message: &str) {
            if let Ok(mut handle) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::temp_dir().join("weasel-typing.log"))
            {
                let _ = writeln!(handle, "{message}");
            }
        }
    }

    /// 按键响应：带回提交文本 → 上屏音；被接受的按键 → 从样本池随机取一个击键音。
    ///
    /// 重放类响应（例如能力变化后恢复内联预编辑）会把 `eaten` 置为 false，
    /// 因此不会误响；被输入法忽略的按键同样不出声。
    pub(super) fn play_for_response(response: &super::KeyEventResponse) {
        let config = config();
        if !config.enabled {
            // 关掉声音时顺手杀掉后台播放器（用户 2026-10-19）
            return;
        }
        // 部署进行中一律静音（用户 2026-10-18 要求）：部署由另一个进程跑，
        // 只能靠用户目录里的 `.deploying` 标记跨进程判断，见 deploy_manifest。
        if deploying() {
            return;
        }
        // 音乐模式：不放打字机键音与上屏铃，但**每个键都要推进歌**
        // （用户 2026-10-19 定的口径；2026-10-21 补充：上屏键也要算 —— 空格提交那一下
        //   原来被 `commit_text.is_empty()` 排除掉了，既不出声也不推进，用户报「空格没声音」）
        if typing_song::music_mode() {
            if response.eaten || !response.commit_text.is_empty() {
                typing_song::step();
                typing_song::start();
            }
            return;
        }
        if !response.commit_text.is_empty() {
            // 用户 2026-10-19：放歌时不要上屏铃声；只有打字机音效模式才响铃。
            if typing_song::settings().is_none() {
                if let Some(path) = config.commit.as_ref() {
                    // 上屏：只播铃的那一声（mp3 用窗口，wav 时窗口被忽略）。
                    play(path, BELL_FROM_MS, BELL_TO_MS);
                }
            }
            return;
        }
        if response.eaten {
            let Some(dir) = runtime_dir() else {
                return;
            };
            // 空格单独一个音（真机空格键杆更长，声音更闷更重）；其余按键走样本池随机。
            let keycode = LAST_KEYCODE.load(std::sync::atomic::Ordering::Relaxed);
            if keycode == 0x20 {
                let space = dir.join("sound").join("typewriter-space.wav");
                if space.exists() {
                    play(&space, 0, 0);
                    return;
                }
            }
            if let Some(path) = pick_key(&config, &dir) {
                // 用户显式配了整首歌（mp3）→ 交给后台循环线程，保证连续播放
                // （否则一曲结束就静音，按键时还会被打断）。
                let is_song = config.key_explicit
                    && path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("mp3"));
                if is_song {
                    {
                        use std::io::Write as _;
                        if let Ok(mut handle) = std::fs::OpenOptions::new().create(true).append(true)
                            .open(std::env::temp_dir().join("weasel-song-trace.log"))
                        {
                            let _ = writeln!(handle, "song-branch path={:?} exists={} explicit={}", path, path.is_file(), config.key_explicit);
                        }
                    }
                    // 打字演奏：累计一次按键，后台线程依次播下一片（不跳段、不起命令行）
                    typing_song::step();
                    typing_song::start();
                    // 打字演奏：每键把位置往前推一小段（脚本自己数文件长度）
                } else {
                    play(&path, 0, 0);
                }
            }
        }
    }
}

/// 将按键响应写回客户端，并附带服务端安全输入策略。
///
/// 队列已关闭等发送错误只记调试日志，不回滚已经完成的引擎状态变更。
fn reply(
    connection: &ClientConnection,
    request_id: u64,
    mut response: KeyEventResponse,
    allow_rime_in_secure_fields: bool,
) {
    response.allow_rime_in_secure_fields = Some(allow_rime_in_secure_fields);
    trace_commit_reply(&response);
    sound::play_for_response(&response);
    if let Err(error) = connection.enqueue(Envelope {
        request_id,
        payload: Some(Payload::KeyEventResponse(host_response(response))),
    }) {
        tracing::debug!(%error, "client response enqueue failed");
    }
}

/// 将协议失败写入连接的发送队列；发送失败不影响引擎状态。
fn failure(connection: &ClientConnection, request_id: u64, code: FailureCode, message: &str) {
    let _ = connection.enqueue(Envelope {
        request_id,
        payload: Some(Payload::Failure(Failure {
            code: code as i32,
            message: message.into(),
        })),
    });
}

/// 输入上下文令牌必须包含非零上下文、连接代次和上下文代次。
fn valid_token(token: Option<&ContextToken>) -> bool {
    token.is_some_and(|token| {
        token.context_id != 0 && token.connection_epoch != 0 && token.generation != 0
    })
}

/// 读取 server 使用的布尔配置；快照异常时记录错误并采用调用方给出的安全默认值。
fn bool_setting_or(
    settings: &weasel_common::settings::ConfigSnapshot,
    path: &str,
    fallback: bool,
) -> bool {
    settings.required::<bool>(path).unwrap_or_else(|error| {
        tracing::error!(%error, path, fallback, "invalid server setting; using fallback");
        fallback
    })
}

impl Engine {
    /// 建一个原生会话并套用当前方案与 ASCII 模式。
    ///
    /// `OpenInput` 与"空闲回收后重建"共用这一条路径，避免两处初始化逻辑漂移。
    fn new_session(
        &self,
        connection: &std::sync::Arc<ClientConnection>,
    ) -> Result<librime::RimeSession, String> {
        let mut session = self.rime.new_session()?;
        if let Some(ascii) = self.global_ascii.or_else(|| {
            self.settings.as_ref().and_then(|settings| {
                settings
                    .app_bool(connection.client_executable().unwrap_or(""), "ascii_mode")
                    .map_err(|error| {
                        tracing::error!(%error, "invalid application setting");
                        error
                    })
                    .ok()
            })
        }) {
            session.set_ascii_mode(ascii);
        }
        // 方案：用户目录 `schema/schema.yaml` 的 `active` 优先。
        //
        // 三条都必须遵守，否则会**把输入打断**（实测：连字母都打不出来 ✗）：
        //   1. 已经是这个方案就**不要**再选 —— 重复 select 会重建会话（丢组合、卡一下）；
        //   2. 选失败只是记日志，绝不改变会话状态（保留 librime 自己的选择）；
        //   3. `select_schema` 自己也做保护：新方案加载失败时保留原会话并返回 false
        //      （`librime-next/src/c_api.rs::rime_select_schema`）。
        //
        // 2026-10-29：这条以前**从来没生效过** —— `Engine/src/librime/api.rs` 的
        // API 表漏装 `select_schema`，函数指针恒为 None ⇒ 永远走 else 分支记 warn。
        // 装上之后才会真的切方案，所以第 1 条"已经一样就跳过"现在很关键，
        // 否则每次焦点进入输入框都会重载一遍方案（仓颉/注音这种大表能明显卡顿）。
        if let Some(active) = self.schemas.active.clone()
            && session.current_schema().as_deref() != Some(active.as_str())
            && !session.select_schema(&active)
        {
            tracing::warn!(
                schema = %active,
                "schema.yaml 里的当前方案不可用，保留 librime 的选择"
            );
        }
        Ok(session)
    }

    /// 确保 client 有可用的原生会话；被空闲回收掉时就地重建。
    ///
    /// 重建只花"建会话 + 选方案"的代价，**不会**重新初始化 librime（那才是
    /// 1010 ms 的大头），所以停顿很久之后的第一键也不会落空。
    fn ensure_session(&mut self, client_id: u64) -> bool {
        let Some(connection) = self
            .clients
            .get(&client_id)
            .map(|client| client.connection.clone())
        else {
            return false;
        };
        if self
            .clients
            .get(&client_id)
            .is_some_and(|client| client.session.is_some())
        {
            return true;
        }
        match self.new_session(&connection) {
            Ok(session) => {
                if let Some(client) = self.clients.get_mut(&client_id) {
                    client.session = Some(session);
                    client.touch();
                }
                true
            }
            Err(error) => {
                tracing::error!(client_id, %error, "session re-creation failed");
                false
            }
        }
    }

    /// 回收超过 [`SESSION_IDLE_RECLAIM`] 没打字的原生会话。
    ///
    /// 只 destroy 会话（释放词库句柄与组合状态），**保留连接、路由与渲染快照**：
    /// 连接是池，会话是池里按需取用的资源。正在组合的一律不动 —— 用户可能只是
    /// 停下来想词，回收会把他打了一半的内容清掉。
    fn reclaim_idle_sessions(&mut self) {
        let now = Instant::now();
        let mut reclaimed = 0_usize;
        for client in self.clients.values_mut() {
            if client.session.is_none() || client.last_response.composing {
                continue;
            }
            if now.duration_since(client.last_typing) >= SESSION_IDLE_RECLAIM {
                client.session = None;
                client.last_response = Default::default();
                reclaimed += 1;
            }
        }
        if reclaimed > 0 {
            tracing::info!(
                reclaimed,
                "idle sessions reclaimed; process and connections stay alive"
            );
        }
    }

    /// 锁定用户数据目录、初始化日志与 Rime，并读取服务级配置。
    ///
    /// 初始化或必需配置读取失败时返回错误；调用者应在专用工作线程中构造实例，
    /// 使后续原生 Rime 操作与初始化处于同一线程。
    pub fn new(
        paths: RuntimePaths,
        renderer: RendererPublisher,
        settings: Option<weasel_common::settings::ConfigSnapshot>,
        theme_inline_preedit: bool,
    ) -> Result<Self, String> {
        let data_lock = crate::data_lock::DataLock::acquire(&paths.user_data)?;
        crate::init_logging(&paths, "server")?;
        let rime = librime::Librime::load(&paths.executable_directory, &paths.user_data)?;
        tracing::info!("librime initialized on engine thread");
        let input_mode_indicator = InputModeIndicatorPolicy::from_settings(settings.as_ref());
        let (global_ascii, allow_rime_in_secure_fields) = match settings.as_ref() {
            Some(settings) => {
                let global_ascii = bool_setting_or(settings, ".global_ascii_status", false)
                    .then(|| bool_setting_or(settings, ".ascii_mode", false));
                let allow_rime_in_secure_fields =
                    bool_setting_or(settings, ".allow_rime_in_secure_fields", false);
                (global_ascii, allow_rime_in_secure_fields)
            }
            None => (None, false),
        };
        // 方案信息：用户目录 schema/schema.yaml（出厂兜底在程序文件夹里）。
        // 2026-10-27 起与引擎侧 `SCHEMA_MANIFEST` 同名，`schema.toml` 不再参与。
        let schema_path = paths
            .user_data
            .join("schema")
            .join(weasel_style::switcher::SCHEMA_TABLE_FILE);
        let schemas = weasel_style::switcher::read_schema_table(
            &weasel_common::rime_layout::data_dir(&paths.executable_directory),
            &paths.user_data,
        );
        if schemas.active.is_some() || !schemas.schemas.is_empty() {
            tracing::info!(
                active = schemas.active.as_deref().unwrap_or(""),
                selected = schemas.selected_ids().len(),
                "loaded schema preference from schema.yaml"
            );
        }
        Ok(Self {
            global_ascii,
            allow_rime_in_secure_fields,
            input_mode_indicator,
            settings,
            theme_inline_preedit,
            schemas,
            schema_path,
            clients: HashMap::new(),
            rime,
            _data_lock: data_lock,
            active_client: None,
            renderer,
            next_session: 1,
            next_mode_indicator: 1,
        })
    }

    /// 校验并处理一个客户端信封，必要时创建、路由或销毁输入会话。
    ///
    /// 过期连接、缺失或无效令牌、未打开的上下文以及跨上下文操作均以协议失败拒绝。
    /// 成功处理会更新会话修订号，并分别向客户端和渲染器发布结果；原生会话操作不得并发。
    fn message(
        &mut self,
        client_id: u64,
        connection: Arc<ClientConnection>,
        alive: Arc<AtomicBool>,
        envelope: Envelope,
    ) {
        if !alive.load(Ordering::Acquire) {
            return;
        }
        let connection_id = client_id;
        let context = match envelope.payload.as_ref() {
            Some(Payload::OpenInput(v)) => v.token.as_ref(),
            Some(Payload::KeyEvent(v)) => v.token.as_ref(),
            Some(Payload::ContextCommand(v)) => v.token.as_ref(),
            _ => None,
        };
        if !valid_token(context) {
            failure(
                &connection,
                envelope.request_id,
                FailureCode::InvalidArgument,
                "input token required",
            );
            return;
        }
        let context_id = context.unwrap().context_id;
        // [Weasel-Next] 会话按**输入上下文**复用，而不是按连接。
        //
        // TIP 在等待超时后会重建 RPC 连接（每条连接都是新的 client_id ✗）；若这里用
        // connection_id 查找，同一个输入框就会"每按一个键新建一个 rime 会话"，
        // 结果是 rime 永远只看到一个孤立字母、组合串积累不起来、候选窗一闪就没 ✓✓
        // （实测：同一个键 j 两次分别得到 `composition='j' 候选=5` 与 `空` ✗）。
        //
        // 因此只看上下文号（外加宿主程序名，避免不同程序串台）；命中旧会话后，
        // 下面会把它的响应通道重新绑定到当前连接。
        let client_executable = connection.client_executable().unwrap_or("").to_owned();
        let existing = self.clients.iter().find_map(|(id, client)| {
            let same_context = client
                .route
                .token
                .as_ref()
                .is_some_and(|t| t.context_id == context_id);
            let same_client =
                client.connection.client_executable().unwrap_or("") == client_executable;
            (same_context && same_client).then_some(*id)
        });
        let client_id = match existing {
            Some(id) => id,
            None if matches!(envelope.payload, Some(Payload::OpenInput(_))) => {
                if self
                    .clients
                    .values()
                    .filter(|c| c.connection_id == connection_id)
                    .count()
                    >= 32
                {
                    failure(
                        &connection,
                        envelope.request_id,
                        FailureCode::InvalidArgument,
                        "too many input contexts",
                    );
                    return;
                }
                let id = self.next_session;
                self.next_session = self
                    .next_session
                    .checked_add(1)
                    .expect("session identity exhausted");
                id
            }
            None => {
                failure(
                    &connection,
                    envelope.request_id,
                    FailureCode::InvalidArgument,
                    "OpenInput required before input",
                );
                return;
            }
        };
        if let Some(Payload::ContextCommand(command)) = envelope.payload.as_ref()
            && command.action == weasel_common::message::ContextAction::Destroy as i32
        {
            if let Some(client) = self.clients.get_mut(&client_id)
                && client.route.observe(command.token.as_ref())
            {
                let response = KeyEventResponse {
                    token: command.token,
                    revision: client.revision + 1,
                    ..Default::default()
                };
                reply(
                    &connection,
                    envelope.request_id,
                    response,
                    self.allow_rime_in_secure_fields,
                );
                if self.active_client == Some(client_id) {
                    self.renderer.publish(RenderSnapshot {
                        session_id: client_id,
                        token: command.token,
                        revision: client.revision + 1,
                        ..Default::default()
                    });
                    self.active_client = None;
                }
                self.clients.remove(&client_id);
                connection.forget_layout(context_id);
                connection.set_input_state(
                    self.clients
                        .values()
                        .any(|c| c.connection_id == connection_id && c.route.focused),
                    self.clients
                        .values()
                        .any(|c| c.connection_id == connection_id && c.last_response.composing),
                );
            } else {
                failure(
                    &connection,
                    envelope.request_id,
                    FailureCode::StaleContext,
                    "stale destroy context",
                );
            }
            return;
        }
        // Only explicit OpenInput allocates a native session. Reopening is idempotent
        // for the same token; context changes must use ContextCommand.
        if let Some(Payload::OpenInput(open)) = envelope.payload.as_ref() {
            if !valid_token(open.token.as_ref()) {
                failure(
                    &connection,
                    envelope.request_id,
                    FailureCode::InvalidArgument,
                    "nonzero input token required",
                );
                return;
            }
            if let Some(client) = self.clients.get(&client_id) {
                if client.route.token != open.token {
                    failure(
                        &connection,
                        envelope.request_id,
                        FailureCode::InvalidArgument,
                        "input already opened; use context commands",
                    );
                    return;
                }
            } else {
                // 会话创建与"套用当前方案/模式"集中在 [`Engine::new_session`]；
                // 空闲回收后的重建走同一条路径，避免两处逻辑漂移。
                let session = match self.new_session(&connection) {
                    Ok(session) => session,
                    Err(error) => {
                        tracing::error!(client_id, %error, "session creation failed");
                        failure(
                            &connection,
                            envelope.request_id,
                            FailureCode::Internal,
                            "session creation failed",
                        );
                        return;
                    }
                };
                self.clients.insert(
                    client_id,
                    ClientSession {
                        // 默认值来自主题（theme.yaml 的 style/inline_preedit）；
                        // app_options.<应用>.inline_preedit 仍然可以逐应用覆盖它。
                        inline_preedit: self
                            .settings
                            .as_ref()
                            .and_then(|settings| {
                                settings
                                    .app_option_bool(
                                        connection.client_executable().unwrap_or(""),
                                        "inline_preedit",
                                    )
                                    .unwrap_or_else(|error| {
                                        tracing::error!(%error, "invalid application setting");
                                        None
                                    })
                            })
                            .unwrap_or(self.theme_inline_preedit),
                        connection_id,
                        connection: connection.clone(),
                        alive,
                        session: Some(session),
                        last_typing: Instant::now(),
                        revision: 0,
                        route: session_route::SessionRoute {
                            token: open.token.clone(),
                            focused: false,
                        },
                        anchor: Default::default(),
                        waiting_for_layout: None,
                        latest_layout_revision: None,
                        pending_mode_indicator: None,
                        last_response: Default::default(),
                    },
                );
            }
            let _ = connection.enqueue(Envelope {
                request_id: envelope.request_id,
                payload: Some(Payload::InputOpened(InputOpened {
                    token: open.token.clone(),
                })),
            });
            return;
        }
        if !self.clients.contains_key(&client_id) {
            if matches!(
                envelope.payload,
                Some(Payload::KeyEvent(_) | Payload::ContextCommand(_))
            ) {
                failure(
                    &connection,
                    envelope.request_id,
                    FailureCode::InvalidArgument,
                    "OpenInput required before input",
                );
            }
            return;
        }
        let token = match envelope.payload.as_ref() {
            Some(Payload::KeyEvent(key)) => Some(key.token.as_ref()),
            Some(Payload::ContextCommand(command)) => Some(command.token.as_ref()),
            _ => None,
        };
        if token.is_some_and(|token| !valid_token(token)) {
            failure(
                &connection,
                envelope.request_id,
                FailureCode::InvalidArgument,
                "nonzero input token required",
            );
            return;
        }
        // 复用到的会话可能属于上一条（已失效的）连接：把响应通道换到当前连接，
        // 否则 rime 的返回会写进一条没人读的管道 ✗（表现是"按键过去了、什么都没回来"）。
        if let Some(client) = self.clients.get_mut(&client_id) {
            if client.connection_id != connection_id {
                tracing::info!(
                    client = client_id,
                    old = client.connection_id,
                    "会话按上下文复用：响应通道已切到当前连接"
                );
                client.connection_id = connection_id;
                client.connection = Arc::clone(&connection);
                client.route.rebind();
            }
        }
        match envelope.payload {
            Some(Payload::KeyEvent(key_event)) => {
                // 空闲回收后本会话可能已被释放；确保有可用会话，否则放弃本次按键。
                if !self.ensure_session(client_id) {
                    failure(
                        &connection,
                        envelope.request_id,
                        FailureCode::Internal,
                        "session unavailable",
                    );
                    return;
                }
                let (response, snapshot) = {
                    let next_mode_indicator = &mut self.next_mode_indicator;
                    let client = self
                        .clients
                        .get_mut(&client_id)
                        .expect("client session exists");
                    if !client.route.observe(key_event.token.as_ref()) {
                        failure(
                            &connection,
                            envelope.request_id,
                            FailureCode::StaleContext,
                            "stale or mismatched key context",
                        );
                        return;
                    } else {
                        let focus_changed = self.active_client != Some(client_id);
                        self.active_client = Some(client_id);
                        client.route.focused = true;
                        // 真实打字活动：刷新"最后打字时间"，避免活跃会话被空闲回收误伤。
                        client.touch();
                        if let Some(ascii) = self.global_ascii {
                            client.session.as_mut().expect("session ensured").set_ascii_mode(ascii);
                        }
                        // [排障] 按键入口：有这行说明按键到了引擎。
                        // 默认关闭：每键 3~4 行同步写盘（tracing-appender 不缓冲），
                        // 实测每键 1~2ms，属于"打字卡"的可去掉成本；排查时设
                        // `WEASEL_ENGINE_VERBOSE=1` 打开。
                        if verbose_diagnostics() {
                            tracing::info!(
                                keycode = key_event.keycode,
                                client = %client_id,
                                "诊断: 收到按键"
                            );
                        }
                        // 乙-1 插桩：`process_key` 的耗时就是宿主侧等待的绝大部分。
                        // 采样开关与 Tip 侧同一个文件，攒够一批才落一次盘。
                        let key_start = std::time::Instant::now();
                        let mut response = client.session.as_mut().expect("session ensured").process_key(&key_event);
                        weasel_common::key_timing::record(
                            key_start.elapsed(),
                            response.eaten,
                            response.composing,
                        );
                        if verbose_diagnostics() {
                            tracing::info!(
                                eaten = response.eaten,
                                composing = response.composing,
                                commit = %response.commit_text,
                                composition = %response.composition,
                                candidates = response.candidates.len(),
                                ascii = ?response.ascii_mode,
                                "诊断: rime 返回"
                            );
                        }
                        if self.global_ascii.is_some()
                            && let Some(ascii) = response.ascii_mode
                        {
                            self.global_ascii = Some(ascii);
                        }
                        // 用户用快捷键/托盘换了方案：记进 schema.toml，下次启动仍然用它。
                        if let Some(current) = client.session.as_mut().expect("session ensured").current_schema() {
                            if self.schemas.active.as_deref() != Some(current.as_str()) {
                                self.schemas.set_active(current.clone());
                                if let Err(error) =
                                    weasel_style::switcher::write_schema_table(&self.schema_path, &self.schemas)
                                {
                                    tracing::warn!(%error, "写回 schema.toml 失败");
                                } else {
                                    tracing::info!(schema = %current, "schema.toml 已记录当前方案");
                                }
                            }
                        }
                        response.external_preedit = response.composing
                            && self.renderer.supports_preedit()
                            && !client.inline_preedit;
                        let previous_visible = client.anchor.valid
                            && (!client.last_response.candidates.is_empty()
                                || (client.last_response.external_preedit
                                    && client.last_response.composing));
                        let previous_ascii = client.last_response.ascii_mode;
                        let anchor_reset = reset_anchor_for_new_composition(
                            &client.last_response,
                            &response,
                            &mut client.anchor,
                        );
                        if response.state_updated {
                            client.revision = client.revision.wrapping_add(1);
                        }
                        response.token = client.route.token.clone();
                        response.revision = client.revision;
                        if anchor_reset {
                            // 临时诊断（2026-10-18 候选窗不显示）：开始等宿主给插入点。
                            if verbose_diagnostics() {
                                tracing::info!(
                                    session_id = client_id,
                                    revision = response.revision,
                                    "诊断: 开始等锚点"
                                );
                            }
                            client.waiting_for_layout = Some(response.revision);
                        } else if response.state_updated && !response.composing {
                            client.waiting_for_layout = None;
                        }
                        if response.state_updated {
                            client.last_response = response.clone();
                            client.last_response.mode_indicator_request_id = None;
                        }
                        if response.ascii_mode.is_some() {
                            client.last_response.ascii_mode = response.ascii_mode;
                        }
                        client.last_response.token = client.route.token.clone();
                        if response.composing {
                            client.pending_mode_indicator = None;
                        } else if previous_ascii
                            .zip(response.ascii_mode)
                            .is_some_and(|(previous, next)| previous != next)
                        {
                            let ascii_mode = response.ascii_mode.expect("checked above");
                            request_mode_indicator(
                                client,
                                &mut response,
                                next_mode_indicator,
                                self.input_mode_indicator,
                                ModeIndicatorReason::UserSwitch,
                                ascii_mode,
                            );
                        }
                        let snapshot = if client.waiting_for_layout.is_some() {
                            // The host edit has not supplied geometry for this
                            // composition. Do not send an intermediate frame
                            // containing new candidates but no matching anchor.
                            (previous_visible
                                || focus_changed
                                || previous_ascii != client.last_response.ascii_mode)
                                .then(|| RenderSnapshot {
                                    session_id: client_id,
                                    revision: client.revision,
                                    token: client.route.token.clone(),
                                    active: true,
                                    ascii_mode: client.last_response.ascii_mode,
                                    ..Default::default()
                                })
                        } else {
                            (response.state_updated || focus_changed).then(|| {
                                render_snapshot(
                                    client_id,
                                    client.revision,
                                    &client.last_response,
                                    &client.anchor,
                                )
                            })
                        };
                        (response, snapshot)
                    }
                };
                reply(
                    &connection,
                    envelope.request_id,
                    response,
                    self.allow_rime_in_secure_fields,
                );
                if let Some(snapshot) = snapshot {
                    self.renderer.publish(snapshot);
                }
            }
            Some(Payload::ContextCommand(command)) => {
                use weasel_common::message::ContextAction;
                // 空闲回收后本会话可能已被释放；确保有可用会话，否则放弃本次命令。
                if !self.ensure_session(client_id) {
                    failure(
                        &connection,
                        envelope.request_id,
                        FailureCode::Internal,
                        "session unavailable",
                    );
                    return;
                }
                let (response, snapshot) = {
                    let next_mode_indicator = &mut self.next_mode_indicator;
                    let client = self
                        .clients
                        .get_mut(&client_id)
                        .expect("client session exists");
                    let action = ContextAction::try_from(command.action)
                        .unwrap_or(ContextAction::Unspecified);
                    if action == ContextAction::Unspecified {
                        failure(
                            &connection,
                            envelope.request_id,
                            FailureCode::InvalidArgument,
                            "unknown context action",
                        );
                        return;
                    }
                    let previous_context =
                        client.route.token.as_ref().map(|token| token.context_id);
                    if !client.route.observe_command(command.token.as_ref(), action) {
                        failure(
                            &connection,
                            envelope.request_id,
                            FailureCode::StaleContext,
                            "stale or mismatched command context",
                        );
                        return;
                    } else {
                        if previous_context
                            != client.route.token.as_ref().map(|token| token.context_id)
                        {
                            client.session.as_mut().expect("session ensured").context_action(ContextAction::Cancel);
                            client.last_response = Default::default();
                            client.anchor = Default::default();
                            client.waiting_for_layout = None;
                            client.latest_layout_revision = None;
                            client.pending_mode_indicator = None;
                        }
                        let was_active = self.active_client == Some(client_id);
                        match action {
                            ContextAction::Focus => {
                                client.route.focused = true;
                                self.active_client = Some(client_id);
                            }
                            ContextAction::Blur | ContextAction::HostTerminated => {
                                client.route.focused = false;
                                if was_active {
                                    self.active_client = None;
                                }
                            }
                            _ => {}
                        }
                        // SetAscii 是旧 TIP 重连时恢复本地记忆的握手，仍按原值确认，
                        // 但不写共享状态；随后的 Focus 再恢复共享值，兼容现有 TIP。
                        if matches!(action, ContextAction::Focus | ContextAction::ToggleAscii)
                            && let Some(ascii) = self.global_ascii
                        {
                            client.session.as_mut().expect("session ensured").set_ascii_mode(ascii);
                        }
                        let previous_ascii = client.last_response.ascii_mode;
                        let mut response = match (action, command.ascii_mode) {
                            (ContextAction::SetAscii, None) => {
                                failure(
                                    &connection,
                                    envelope.request_id,
                                    FailureCode::InvalidArgument,
                                    "SetAscii requires ascii_mode",
                                );
                                return;
                            }
                            (ContextAction::SetAscii, Some(ascii)) => {
                                client.session.as_mut().expect("session ensured").set_ascii_mode_response(ascii)
                            }
                            _ => client.session.as_mut().expect("session ensured").context_action(action),
                        };
                        if action == ContextAction::ToggleAscii
                            && self.global_ascii.is_some()
                            && let Some(ascii) = response.ascii_mode
                        {
                            self.global_ascii = Some(ascii);
                        }
                        response.external_preedit = response.composing
                            && self.renderer.supports_preedit()
                            && !client.inline_preedit;
                        client.revision = client.revision.wrapping_add(1);
                        response.token = client.route.token.clone();
                        response.revision = client.revision;
                        if matches!(
                            action,
                            ContextAction::Blur
                                | ContextAction::Cancel
                                | ContextAction::Submit
                                | ContextAction::HostTerminated
                        ) || (response.state_updated && !response.composing)
                        {
                            client.waiting_for_layout = None;
                        }
                        if matches!(
                            action,
                            ContextAction::Blur
                                | ContextAction::Cancel
                                | ContextAction::Submit
                                | ContextAction::HostTerminated
                        ) || response.composing
                        {
                            client.pending_mode_indicator = None;
                        }
                        if response.state_updated {
                            client.last_response = response.clone();
                            client.last_response.mode_indicator_request_id = None;
                        }
                        if response.ascii_mode.is_some() {
                            client.last_response.ascii_mode = response.ascii_mode;
                        }
                        // Focus acknowledgements must not replay the previous commit.
                        client.last_response.token = client.route.token.clone();
                        let indicator = match action {
                            ContextAction::Focus => response
                                .ascii_mode
                                .map(|ascii| (ModeIndicatorReason::Focus, ascii)),
                            ContextAction::ToggleAscii => previous_ascii
                                .zip(response.ascii_mode)
                                .filter(|(previous, next)| previous != next)
                                .map(|(_, ascii)| (ModeIndicatorReason::UserSwitch, ascii)),
                            _ => None,
                        };
                        if let Some((reason, ascii_mode)) = indicator {
                            request_mode_indicator(
                                client,
                                &mut response,
                                next_mode_indicator,
                                self.input_mode_indicator,
                                reason,
                                ascii_mode,
                            );
                        }
                        let snapshot = if self.active_client == Some(client_id) {
                            Some(render_snapshot(
                                client_id,
                                client.revision,
                                &client.last_response,
                                &client.anchor,
                            ))
                        } else if was_active {
                            Some(RenderSnapshot {
                                session_id: client_id,
                                revision: client.revision,
                                token: client.route.token.clone(),
                                ..Default::default()
                            })
                        } else {
                            None
                        };
                        (response, snapshot)
                    }
                };
                reply(
                    &connection,
                    envelope.request_id,
                    response,
                    self.allow_rime_in_secure_fields,
                );
                if let Some(snapshot) = snapshot {
                    self.renderer.publish(snapshot);
                }
            }
            _ => (),
        }
    }

    /// 仅将匹配当前活动会话令牌和修订号的渲染器操作交给 Rime。
    ///
    /// 断开连接、失焦或版本过期的事件会被忽略；有效操作递增会话修订号并发布新状态。
    fn renderer_event(&mut self, event: RendererEvent) {
        let Some(client) = self.clients.get_mut(&event.session_id) else {
            tracing::debug!(
                session_id = event.session_id,
                "ignored renderer event for disconnected client"
            );
            return;
        };
        if self.active_client != Some(event.session_id)
            || !client.alive.load(Ordering::Acquire)
            || !client.route.accepts_ui(&event, client.revision)
        {
            return;
        }
        let Some(session) = client.session.as_mut() else {
            tracing::debug!(
                session_id = event.session_id,
                "renderer event for reclaimed session; ignored"
            );
            return;
        };
        let mut response = session.process_renderer_event(&event);
        response.external_preedit =
            response.composing && self.renderer.supports_preedit() && !client.inline_preedit;
        client.revision = client.revision.wrapping_add(1);
        response.token = client.route.token.clone();
        response.revision = client.revision;
        if !response.composing {
            client.waiting_for_layout = None;
        }
        client.last_response = response.clone();
        let snapshot =
            render_snapshot(event.session_id, client.revision, &response, &client.anchor);
        let connection = Arc::clone(&client.connection);
        reply(&connection, 0, response, self.allow_rime_in_secure_fields);
        self.renderer.publish(snapshot);
    }
}

impl Processor<Work> for Engine {
    /// 在工作线程串行分派客户端消息和渲染器事件。
    ///
    /// 消息处理后先重算连接的输入活跃度，再释放请求租约，以免回收器观察到过期状态。
    fn process(&mut self, work: Work) {
        match work {
            Work::Message {
                client_id,
                connection,
                alive,
                envelope,
                _request,
            } => {
                self.message(client_id, connection, alive, envelope);
                // Reclassify input before dropping the request's eviction guard.
                self.idle();
                drop(_request);
            }
            Work::ReclaimIdle => self.reclaim_idle_sessions(),
            Work::Renderer(event) => self.renderer_event(event),
        }
    }

    /// 处理能力变化、最新宿主布局和断连会话的清理。
    ///
    /// 每轮仅消费活动上下文的最新布局；版本不匹配的几何不会解除新组合的等待状态。
    fn idle(&mut self) {
        // Capability changes wake this worker. Restore inline text immediately
        // after a renderer disconnect; do not wait for the next keystroke.
        if !self.renderer.supports_preedit() {
            for (id, client) in &mut self.clients {
                if client.last_response.external_preedit && client.alive.load(Ordering::Acquire) {
                    client.last_response.external_preedit = false;
                    client.revision = client.revision.wrapping_add(1);
                    client.last_response.revision = client.revision;
                    let mut response = client.last_response.clone();
                    response.commit_text.clear();
                    response.open_emoji_panel = false;
                    response.state_updated = true;
                    response.eaten = false;
                    reply(
                        &client.connection,
                        0,
                        response,
                        self.allow_rime_in_secure_fields,
                    );
                    if self.active_client == Some(*id) {
                        self.renderer.publish(render_snapshot(
                            *id,
                            client.revision,
                            &client.last_response,
                            &client.anchor,
                        ));
                    }
                }
            }
        }
        // Poll the active connection's latest geometry, never the input FIFO.
        // Layout arrival explicitly wakes the engine, including the final drag position.
        if let Some(client_id) = self.active_client
            && let Some(client) = self.clients.get_mut(&client_id)
            && client.alive.load(Ordering::Acquire)
            && let Some(update) = client
                .connection
                .take_layout_for(client.route.token.as_ref())
        {
            if let Some(request_id) = update.mode_indicator_request_id {
                if let Some(pending) = client
                    .pending_mode_indicator
                    .filter(|pending| pending.id == request_id)
                {
                    client.pending_mode_indicator = None;
                    let now = Instant::now();
                    let anchor = update.anchor.unwrap_or_default();
                    if now < pending.expires_at && anchor.valid && !client.last_response.composing {
                        let mut snapshot = render_snapshot(
                            client_id,
                            client.revision,
                            &client.last_response,
                            &anchor,
                        );
                        snapshot.mode_indicator = Some(ModeIndicator {
                            id: pending.id,
                            ascii_mode: pending.ascii_mode,
                            reason: pending.reason as i32,
                        });
                        self.renderer.publish(snapshot);
                    }
                }
            } else {
                let matching = layout_matches_pending(
                    update.revision,
                    client.waiting_for_layout,
                    client.latest_layout_revision,
                );
                // 临时诊断（2026-10-18 候选窗不显示）：布局更新到底来没来、为什么被判不合格。
                if verbose_diagnostics() {
                    tracing::info!(
                        session_id = client_id,
                        revision = ?update.revision,
                        waiting = ?client.waiting_for_layout,
                        accepted = ?client.latest_layout_revision,
                        matching,
                        anchor_valid = ?update.anchor.as_ref().map(|anchor| anchor.valid),
                        "诊断: 布局更新"
                    );
                }
                if matching {
                    if let Some(revision) = update.revision {
                        client.latest_layout_revision = Some(revision);
                    }
                    let anchor = update.anchor.unwrap_or_default();
                    let became_ready = client.waiting_for_layout.is_some() && anchor.valid;
                    if became_ready {
                        client.waiting_for_layout = None;
                    }
                    if client.anchor != anchor || became_ready {
                        client.anchor = anchor;
                        if client.waiting_for_layout.is_none()
                            && (!client.last_response.candidates.is_empty()
                                || client.last_response.external_preedit)
                        {
                            self.renderer.publish(render_snapshot(
                                client_id,
                                client.revision,
                                &client.last_response,
                                &client.anchor,
                            ));
                        }
                    }
                }
            }
        }
        let hide = self.active_client.and_then(|client_id| {
            self.clients
                .get(&client_id)
                .filter(|client| !client.alive.load(Ordering::Acquire))
                .map(|client| RenderSnapshot {
                    session_id: client_id,
                    revision: client.revision.wrapping_add(1),
                    token: client.route.token.clone(),
                    ..Default::default()
                })
        });
        self.clients
            .retain(|_, client| client.alive.load(Ordering::Acquire));
        let mut connections = HashMap::new();
        for client in self.clients.values() {
            let entry = connections.entry(client.connection_id).or_insert((
                &client.connection,
                false,
                false,
            ));
            entry.1 |= client.route.focused;
            entry.2 |= client.last_response.composing;
        }
        for (connection, focused, composing) in connections.values() {
            connection.set_input_state(*focused, *composing);
        }
        if let Some(client_id) = self.active_client {
            if !self.clients.contains_key(&client_id) {
                self.active_client = None;
                if let Some(hide) = hide {
                    self.renderer.publish(hide);
                }
            }
        }
    }
}

#[cfg(test)]
mod sound_parse_tests {
    use super::sound::parse_for_test;
    use super::sound::parse_merged_for_test;
    use std::fs;

    /// 相对 asset 应从 exe 目录逐级向上找同名目录，子路径相对它解析。
    #[test]
    fn sound_parse_resolves_relative_asset() {
        let root = std::env::temp_dir().join(format!("weasel-asset-{}", std::process::id()));
        let exe_dir = root.join("artifact").join("smoke");
        let sound_dir = root.join("Rime-Next").join("sound");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&sound_dir).unwrap();
        fs::write(sound_dir.join("typewriter-bell.wav"), b"x").unwrap();
        fs::write(sound_dir.join("typewriter-key.wav"), b"x").unwrap();

        // ⚠️ 配置只有 YAML 一种格式：下面必须是合法 YAML（`key: value`）。
        // 2026-10-23 起 TOML 连兼容读都没有了，喂 TOML 会静默读不到值。
        let config_text = r#"
asset: Rime-Next
sound:
  enabled: true
  key: sound/typewriter-key.wav
  commit: sound/typewriter-bell.wav
"#;
        let (enabled, key, commit) = parse_for_test(config_text, &exe_dir);
        assert!(enabled, "enabled 应为 true");
        let key = key.expect("key 应解析出路径");
        let commit = commit.expect("commit 应解析出路径");
        assert_eq!(key, sound_dir.join("typewriter-key.wav"), "key 解析错误");
        assert_eq!(commit, sound_dir.join("typewriter-bell.wav"), "commit 解析错误");
        assert!(commit.is_file(), "commit 解析出的文件应真实存在: {commit:?}");
        let _ = fs::remove_dir_all(&root);
    }

    /// 用户在设置界面里关掉声音：用户目录那份必须盖掉程序那份的 `enabled: true`。
    ///
    /// 2026-10-18 实际踩到：界面写了 `weasel.yaml` 的 `enabled = false`，
    /// 引擎却只读程序文件夹的旧配置（`enabled = true`）⇒ 开关点了没用。
    #[test]
    fn user_custom_disables_sound_over_program_default() {
        let host = "sound:\n  enabled: true\n  key: sound/typewriter-key.wav\n";
        let user = "sound:\n  enabled: false\n";
        let (enabled, key, _commit) = parse_merged_for_test(host, user);
        assert!(!enabled, "用户 weasel.yaml 的 enabled: false 必须生效");
        assert_eq!(
            key.unwrap(),
            std::path::PathBuf::from("sound/typewriter-key.wav"),
            "用户没写的字段应沿用程序那份"
        );
    }

    /// 未配置 asset 时退回 exe 目录（安装后的布局）。
    #[test]
    fn sound_parse_falls_back_to_exe_directory() {
        let exe_dir = std::env::temp_dir().join(format!("weasel-noasset-{}", std::process::id()));
        let _ = fs::remove_dir_all(&exe_dir);
        fs::create_dir_all(exe_dir.join("sound")).unwrap();
        let config_text = r#"
sound:
  enabled: true
  key: sound/k.wav
  commit: sound/b.wav
"#;
        let (_, key, commit) = parse_for_test(config_text, &exe_dir);
        assert_eq!(key.unwrap(), exe_dir.join("sound").join("k.wav"));
        assert_eq!(commit.unwrap(), exe_dir.join("sound").join("b.wav"));
        let _ = fs::remove_dir_all(&exe_dir);
    }
}
