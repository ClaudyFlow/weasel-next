//! 独立的外观预览入口与示例渲染数据。
//!
//! 使用 `Weasel-Next-Renderer.exe --preview` 时，程序从合成的候选快照渲染当前配置的主题，
//! 并在任务栏显示可手动关闭的预览窗口。主题设置仍从 broker 读取，但此模式不连接
//! server，也不监听实时 renderer 管道；候选项仅用于展示，不会触发真实输入交互。
use crate::theme_api::UiMode;
use weasel_common::message::{RenderItem, RenderRect, RenderSnapshot};

/// 合成预览条带使用的连接所有者编号。
///
/// 该固定编号用于构造有效的演示快照，不对应真实输入会话。
pub(crate) const PREVIEW_OWNER: u64 = 1;

/// 将命令行参数解析为 renderer 的运行模式。
///
/// 不带参数时使用实时模式；仅接受单个 `--preview` 参数进入预览模式。预览主题仍由
/// broker 配置决定，此解析过程不提供主题或输入模式覆盖。
///
/// # 错误
///
/// 参数组合不受支持时返回用法提示。
pub fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<UiMode, String> {
    let args: Vec<_> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(UiMode::Live),
        [arg] if arg == "--preview" => Ok(UiMode::Preview),
        [arg] if arg == "--preview-indicator" => {
            INDICATOR_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(UiMode::Preview)
        }
        // 「组字中切中英」的样本：候选 + 预编辑 + mode_indicator 三者同时在。
        [arg] if arg == "--preview-badge" => {
            BADGE_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(UiMode::Preview)
        }
        // 「上屏后联想」样本：无组字、无 preedit，只有 4 条预测词
        // （2026-11-08 用户口径：联想条 3 → 4 条，数字键 1/2/3/4）。
        [arg] if arg == "--preview-predict" => {
            PREDICT_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(UiMode::Preview)
        }
        // 「打字时的预测候选」样本：普通候选 + 带「预测」标签的预测候选混在同一帧。
        // 用来核对 2026-11-08 用户口径「预测这个标签做成小一号，预测的候选字号不变」：
        // 只有 secondary_text（comment「预测」）用半字号，primary_text 与普通候选同字号。
        [arg] if arg == "--preview-predict-tag" => {
            PREDICT_TAG_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(UiMode::Preview)
        }
        _ => Err(
            "usage: Weasel-Next-Renderer.exe [--preview|--preview-indicator]".into(),
        ),
    }
}

/// 预览是否只显示中英模式提示窗（`--preview-indicator`）。
///
/// 模式提示平时只闪现 800 毫秒，没法截图核对；这个开关让预览直接停在提示窗上。
static INDICATOR_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 预览"组字中切中英"的徽标样本（有候选 + 有预编辑 + 带 mode_indicator）。
///
/// 这条路径 2026-11-02 之前一直是坏的：主题一看到 `mode_indicator` 就只画徽标、
/// 候选一个不画（用户当场："候选窗完全不显示"）。现在有它才测得到。
static BADGE_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 预览「上屏后联想」样本：无组字、无 preedit，只有引擎给的预测词。
///
/// 2026-11-07 用户要求「预测一些上屏十次以上的用户词」——上屏后这条窗口就是
/// 联想条的样子，靠它截图核对（真实链路要等引擎侧 predictions 接上）。
static PREDICT_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 预览「打字时的预测候选」样本：普通候选与带「预测」标签的预测候选同帧。
///
/// 2026-11-08 用户纠正口径：「预测这个标签做成小一号，预测的候选字号不变」。
/// 真实链路里预测候选由 librime 的 `NeuralPredictFilter` 追加，`comment == "预测"`，
/// `Renderer::render_snapshot` 据此把它标成 `kind = "prediction"`。
static PREDICT_TAG_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 构造用于展示主题各部分的合成候选快照。
///
/// 内容刻意与官方候选窗的观感对照：首屏、第一个候选高亮、其中几条带注释，
/// 这样"我们的候选窗长什么样"可以和官方截图逐像素比（见 `script\measure-candidate.py`）。
pub fn synthetic_snapshot() -> RenderSnapshot {
    if INDICATOR_ONLY.load(std::sync::atomic::Ordering::Relaxed) {
        return mode_indicator_snapshot();
    }
    if BADGE_ONLY.load(std::sync::atomic::Ordering::Relaxed) {
        return badge_snapshot();
    }
    if PREDICT_ONLY.load(std::sync::atomic::Ordering::Relaxed) {
        return prediction_snapshot();
    }
    if PREDICT_TAG_ONLY.load(std::sync::atomic::Ordering::Relaxed) {
        return prediction_tag_snapshot();
    }
    candidate_snapshot()
}

/// 「上屏后联想」样本：无组字、无 preedit，只有 4 条预测词（与引擎 `render_snapshot`
/// 在 `response.candidates` 为空时的产出同形：`kind = "prediction"`、索引 0、不可翻页）。
///
/// 2026-11-08 用户口径：联想条 3 → 4 条（数字键 1/2/3 变 1/2/3/4）。
fn prediction_snapshot() -> RenderSnapshot {
    let mut snapshot = candidate_snapshot();
    snapshot.preedit = None;
    snapshot.items = ["现在", "这个", "然后", "应该"]
        .into_iter()
        .map(|text| RenderItem {
            primary_text: text.into(),
            secondary_text: String::new(),
            enabled: true,
            kind: "prediction".into(),
        })
        .collect();
    snapshot.selected_index = 0;
    snapshot.page_start = 0;
    snapshot.total_item_count = None;
    snapshot.can_page_previous = false;
    snapshot.can_page_next = false;
    snapshot
}

/// 「打字时的预测候选」样本：普通候选 + 预测候选混排，带预编辑。
///
/// 预测候选的 `secondary_text` 就是「预测」标签（引擎侧 `comment == "预测"`）；
/// 主题应只把这个标签画小一号，prediction 的 `primary_text` 与普通候选同字号。
fn prediction_tag_snapshot() -> RenderSnapshot {
    let mut snapshot = candidate_snapshot();
    snapshot.preedit = Some(weasel_common::message::RenderPreedit {
        text: "jie".into(),
        cursor_utf16: 3,
    });
    let samples = [
        ("界面", "jiemian", "candidate"),
        ("解谜", "jiemi", "candidate"),
        ("你好", "预测", "prediction"),
        ("你们", "预测", "prediction"),
    ];
    snapshot.items = samples
        .into_iter()
        .map(|(primary, secondary, kind)| RenderItem {
            primary_text: primary.into(),
            secondary_text: secondary.into(),
            enabled: true,
            kind: kind.into(),
        })
        .collect();
    snapshot.selected_index = 0;
    snapshot.page_start = 0;
    snapshot.total_item_count = Some(4);
    snapshot.can_page_previous = false;
    snapshot.can_page_next = false;
    snapshot
}

/// 候选窗样本：与外观对照用的五条候选（不读 `INDICATOR_ONLY`）。
///
/// 2026-11-02 02:2x：原来自测台的 `mode_indicator_snapshot()` 直接调
/// `synthetic_snapshot()`，而 `synthetic_snapshot()` 在 `--preview-indicator` 下又调回
/// `mode_indicator_snapshot()` —— **无限递归**，进程直接 `has overflowed its stack`。
/// 所以"中英提示"这条自测台一直是坏的，谁都验证不了。拆出这个不递归的构造函数。
fn candidate_snapshot() -> RenderSnapshot {
    let samples = [
        ("界面", "jiemian"),
        ("解谜", "jiemi"),
        ("节目", "jiemu"),
        ("姐妹", "jiemei"),
        ("解密", "jiemi"),
    ];
    let items = samples
        .into_iter()
        .map(|(primary, secondary)| RenderItem {
            primary_text: primary.into(),
            secondary_text: secondary.into(),
            enabled: true,
            kind: "candidate".into(),
        })
        .collect();
    RenderSnapshot {
        preedit: None,
        active: true,
        ascii_mode: Some(false),
        mode_indicator: None,
        visible: true,
        sequence: 1,
        session_id: 1,
        revision: 1,
        // The preview positions itself on the primary monitor and ignores the
        // anchor; it only needs to be valid to satisfy the visibility rule.
        anchor: Some(RenderRect {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
            valid: true,
        }),
        items,
        selected_index: 0,
        page_start: 0,
        total_item_count: Some(20),
        can_page_previous: false,
        can_page_next: true,
        token: None,
    }
}

/// 启动独立预览窗口。
///
/// `_mode` 为统一 renderer 入口保留；预览窗口自身负责从 broker 加载主题设置。
///
/// # 错误
///
/// 窗口类、窗口控件或工作线程初始化失败时返回错误文本。
pub fn run(_mode: UiMode) -> Result<(), String> {
    crate::preview_window::run()
}

/// 组字中切中英的徽标样本。
fn badge_snapshot() -> RenderSnapshot {
    let mut snapshot = candidate_snapshot();
    snapshot.preedit = Some(weasel_common::message::RenderPreedit {
        text: "jie m".into(),
        cursor_utf16: 5,
    });
    snapshot.ascii_mode = Some(true);
    snapshot.mode_indicator = Some(weasel_common::message::ModeIndicator {
        id: 1,
        ascii_mode: true,
        reason: weasel_common::message::ModeIndicatorReason::UserSwitch as i32,
        caps_lock: false,
    });
    snapshot
}

/// 只含模式提示的合成快照：走主题的 `Layout::mode_indicator` 那条路。
fn mode_indicator_snapshot() -> RenderSnapshot {
    let mut snapshot = candidate_snapshot();
    snapshot.items.clear();
    snapshot.preedit = None;
    snapshot.selected_index = 0;
    snapshot.visible = false;
    // 主题画提示读的是 `snapshot.ascii_mode`（不是 mode_indicator 里那个），
    // 两处都写，预览和真实链路才一致。
    snapshot.ascii_mode = Some(true);
    snapshot.mode_indicator = Some(weasel_common::message::ModeIndicator {
        id: 1,
        ascii_mode: true,
        reason: weasel_common::message::ModeIndicatorReason::UserSwitch as i32,
        caps_lock: false,
    });
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_are_strict() {
        let parse = |args: &[&str]| parse_args(args.iter().map(std::ffi::OsString::from));
        assert_eq!(parse(&[]).unwrap(), UiMode::Live);
        assert_eq!(parse(&["--preview"]).unwrap(), UiMode::Preview);
        assert_eq!(parse(&["--preview-badge"]).unwrap(), UiMode::Preview);
        assert_eq!(parse(&["--preview-predict"]).unwrap(), UiMode::Preview);
        assert_eq!(parse(&["--preview-predict-tag"]).unwrap(), UiMode::Preview);
        assert!(parse(&["--preview", "--preedit"]).is_err());
        assert!(parse(&["--preedit"]).is_err());
        assert!(parse(&["--preview", "--preview"]).is_err());
        assert!(parse(&["--preveiw"]).is_err());
        assert!(parse(&["--preview", "ten"]).is_err());
    }

    #[test]
    fn synthetic_snapshot_is_valid_and_visible() {
        let snapshot = synthetic_snapshot();
        crate::state::validate(&snapshot).unwrap();
        assert!(crate::presentation::is_visible(
            &crate::theme_adapter::view(&snapshot, 1)
        ));
        // 预览样本是"第一页"：上一页不可用、下一页可用（与官方候选窗观感一致）
        assert!(!snapshot.can_page_previous && snapshot.can_page_next);
        assert_eq!(snapshot.page_start, 0, "第一页的序号从 1 开始");
        assert_eq!(snapshot.selected_index, 0);
        assert!((snapshot.selected_index as usize) < snapshot.items.len());
    }
}
