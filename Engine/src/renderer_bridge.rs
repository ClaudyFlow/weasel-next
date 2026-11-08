//! 在引擎与渲染器之间传递最新渲染快照，并将渲染器事件送回引擎。
//!
//! 快照保存在容量为一的 watch 通道中，慢速或暂时离线的渲染器只会收到最新状态，
//! 不会积压过时网络消息。渲染器能力协商结果通过发布器共享给引擎。
use crate::engine::Work;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
use weasel_common::{
    message::{QueryConfig, RenderItem, RenderRect, RenderSnapshot, envelope::Payload},
    rpc::{RpcClient, RpcError, default_renderer_pipe_name},
};

/// 查询渲染器是否支持外置预编辑区。
///
/// 只接受配置响应中严格等于 JSON `true` 的值；其他响应类型视为 RPC 协议错误。
async fn query_preedit_capability(client: &RpcClient) -> Result<bool, RpcError> {
    let response = client
        .request(Payload::QueryConfig(QueryConfig {
            refresh: false,
            path: ".capabilities.preedit".into(),
        }))
        .await?;
    match response.payload {
        Some(Payload::ConfigValue(value)) => Ok(value.json.as_deref() == Some("true")),
        _ => Err(RpcError::UnexpectedResponse),
    }
}

#[derive(Clone)]
/// 在线程间共享的渲染状态发布端。
///
/// 克隆句柄共享能力标志、最新快照槽和单调序号；写入采用覆盖语义，不为慢接收方
/// 保存历史快照。
pub(crate) struct RendererPublisher {
    /// 当前渲染器是否支持外置预编辑区，供引擎以 Acquire/Release 顺序读取。
    preedit: Arc<AtomicBool>,
    /// 容量为一的最新快照槽；所有发布者克隆共享同一通道。
    snapshots: watch::Sender<Option<RenderSnapshot>>,
    /// 为所有发布操作分配唯一递增序号，达到上限时拒绝继续发布。
    sequence: Arc<AtomicU64>,
}
impl RendererPublisher {
    /// 创建发布器及其唯一订阅端；订阅端初始尚无快照。
    pub fn channel() -> (Self, watch::Receiver<Option<RenderSnapshot>>) {
        let (tx, rx) = watch::channel(None);
        (
            Self {
                preedit: Arc::new(AtomicBool::new(false)),
                snapshots: tx,
                sequence: Arc::new(AtomicU64::new(0)),
            },
            rx,
        )
    }
    /// 覆盖发布最新快照，并由发布器统一分配序号。
    ///
    /// 序号分配与 watch 槽更新在同一写锁内完成，以保证并发克隆发布后，槽中的快照
    /// 不会出现序号倒退；`u64` 耗尽时触发 panic，避免序号回绕产生歧义。
    pub fn publish(&self, mut snapshot: RenderSnapshot) {
        // Allocate while holding the watch slot's write lock so concurrent clones
        // cannot publish an older sequence after a newer one. Never wrap to zero.
        self.snapshots.send_modify(|latest| {
            snapshot.sequence = self
                .sequence
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .expect("renderer publication sequence exhausted")
                + 1;
            *latest = Some(snapshot);
        });
    }
    /// 返回最近一次完成的渲染器能力协商结果。
    pub fn supports_preedit(&self) -> bool {
        self.preedit.load(Ordering::Acquire)
    }
}

/// 启动渲染器桥接任务。
///
/// `eager` 为真时连接与能力查询立即开始；否则仅在存在可见快照后连接。任务负责
/// 重连、双向转发和更新能力标志；调用方可取消并等待返回的句柄以完成清理。
pub(crate) fn spawn(
    mut snapshots: watch::Receiver<Option<RenderSnapshot>>,
    engine: crate::worker::Sender<Work>,
    publisher: RendererPublisher,
    eager: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // External preedit needs capability negotiation before the first key.
            // Inline-only sessions retain the lazy, visible-snapshot connection.
            if !eager
                && snapshots
                    .borrow()
                    .as_ref()
                    .is_none_or(|s| !s.visible && s.mode_indicator.is_none())
            {
                if snapshots.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let client = match RpcClient::connect_as(
                default_renderer_pipe_name(),
                weasel_common::message::PeerRole::Server,
            )
            .await
            {
                Ok(client) => client,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    continue;
                }
            };
            let supported = match tokio::time::timeout(
                Duration::from_secs(2),
                query_preedit_capability(&client),
            )
            .await
            {
                Ok(Ok(value)) => value,
                _ => false,
            };
            publisher.preedit.store(supported, Ordering::Release);
            (engine.notifier())();
            let mut events = client.subscribe();
            let mut dirty = true;
            // One connection/task owns both directions; reconnect drops its subscription.
            loop {
                let latest = if dirty {
                    snapshots.borrow_and_update().clone()
                } else {
                    None
                };
                dirty = false;
                if let Some(snapshot) = latest {
                    if client.publish(Payload::RenderSnapshot(snapshot)).is_err() {
                        break;
                    }
                }
                tokio::select! {
                    changed = snapshots.changed() => { if changed.is_err() { client.disconnect().await; return; } dirty = true; }
                    event = events.recv() => match event {
                        Ok(envelope) => {
                            if let Some(Payload::RendererEvent(event)) = envelope.payload {
                                let _ = engine.try_send(Work::Renderer(event));
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => (),
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    _ = client.disconnected() => break,
                }
            }
            publisher.preedit.store(false, Ordering::Release);
            (engine.notifier())();
            client.disconnect().await;
        }
    })
}

/// librime 追加的预测候选的注释标记（宿主侧唯一的识别口径）。
///
/// 打字时 librime 的预测过滤器会给候选追加 `comment == "预测"`（精确字符串，
/// 见 librime-next `src/engine/gear/neural/predict_filter.rs` 的
/// `Candidate::new("predict", ...).with_comment("预测")`）。宿主**不扩 ABI、不新增
/// 候选字段**，只用这个现成注释区分「预测候选」与普通候选：匹配到的项在渲染快照里
/// 标 `kind = "prediction"`，theme 据此把「预测」标签换成小号字体
/// （2026-11-08 用户：「预测这个标签做成小一号，预测的候选字号不变」）。
///
/// 必须**精确相等**才算：其它带「预测」二字的注释（如「预测词」）一律按普通候选。
const PREDICTION_COMMENT: &str = "预测";

/// 根据一次引擎按键响应构造渲染快照。
///
/// 快照序号保留为零，由 [`RendererPublisher::publish`] 在真正发布时分配。候选项和
/// 分页状态沿用引擎响应；候选窗只有在存在候选项或外置预编辑内容且锚点有效时可见。
///
/// `strip` 是引擎维护的**联想条显式状态**（见 `engine.rs::update_prediction_strip`），
/// 不是 `response.predictions` 的直接投影。两个理由：
/// - librime 的 `Context.predictions` 由 `apply_commit` 填完就没人清，c_api 只要
///   无组字就照常导出 ⇒ 上屏之后的**每一帧**都带着同一批旧预测。若在这里直接读
///   `response.predictions`，联想条每帧都会被画回来（2026-11-08 用户报「关不掉」）。
/// - 反过来，只在"提交那一帧"读 predictions 也不行：提交键的**抬起帧**紧随其后，
///   它把 `commit_text` 冲成空 ⇒ 联想条"闪一下没了"（2026-11-08 用户报「我怎么选」）。
///   所以可见性必须由一个跨帧保持的状态决定，而不是由当前帧的字段决定。
pub(crate) fn render_snapshot(
    session_id: u64,
    revision: u64,
    response: &weasel_common::message::KeyEventResponse,
    strip: &[String],
    anchor: &RenderRect,
) -> RenderSnapshot {
    let items: Vec<RenderItem> = if response.candidates.is_empty() {
        strip
            .iter()
            .map(|text| RenderItem {
                primary_text: text.clone(),
                secondary_text: String::new(),
                enabled: true,
                kind: "prediction".to_owned(),
            })
            .collect()
    } else {
        response
            .candidates
            .iter()
            .map(|candidate| RenderItem {
                primary_text: candidate.text.clone(),
                secondary_text: candidate.comment.clone(),
                enabled: true,
                // 打字时的预测候选靠 comment 精确等于「预测」识别（最小改动口径，
                // 不扩 ABI）：只有这些项标 "prediction" 让 theme 缩「预测」标签；
                // 其余（含空注释/拼音注释）仍是普通 "candidate"，字号/语义不动。
                kind: if candidate.comment == PREDICTION_COMMENT {
                    "prediction".to_owned()
                } else {
                    "candidate".to_owned()
                },
            })
            .collect()
    };
    RenderSnapshot {
        sequence: 0, // Assigned only when published; independent of input revision.
        session_id,
        revision,
        active: true,
        ascii_mode: response.ascii_mode,
        // 可见性只由候选/组字/联想内容决定，不再依赖锚点是否有效。
        // 锚点无效（新组合首键、方案選單等尚无光标上下文）时窗口仍应出现，
        // 沿用上一次的有效锚点兜底定位，待布局到达后再重定位——否则候选窗
        // 与 Ctrl+` 方案選單会整帧被扣住而永远看不见。
        visible: !items.is_empty() || (response.external_preedit && response.composing),
        preedit: (response.external_preedit && response.composing).then(|| {
            weasel_common::message::RenderPreedit {
                text: response.composition.clone(),
                cursor_utf16: response.composition_cursor,
            }
        }),
        anchor: Some(anchor.clone()),
        items,
        selected_index: if response.candidates.is_empty() {
            0
        } else {
            response.selected_candidate
        },
        page_start: response.page_start,
        total_item_count: None,
        can_page_previous: response.can_page_previous,
        can_page_next: response.can_page_next,
        token: response.token.clone(),
        mode_indicator: None,
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn clones_share_sequence_independent_of_context_and_input_revision() {
        let (publisher, mut receiver) = RendererPublisher::channel();
        let clone = publisher.clone();
        for (index, sender) in [&publisher, &clone, &publisher].into_iter().enumerate() {
            sender.publish(RenderSnapshot {
                sequence: 999, // Caller values are always overwritten.
                session_id: index as u64 + 1,
                revision: 7,
                visible: index != 2,
                ..Default::default()
            });
            let latest = receiver.borrow_and_update();
            let latest = latest.as_ref().unwrap();
            assert_eq!(latest.sequence, index as u64 + 1);
            assert_eq!(latest.revision, 7);
        }
    }

    #[test]
    fn concurrent_clones_cannot_replace_latest_with_an_older_sequence() {
        let (publisher, receiver) = RendererPublisher::channel();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let publisher = publisher.clone();
                scope.spawn(move || {
                    for _ in 0..100 {
                        publisher.publish(RenderSnapshot::default());
                    }
                });
            }
        });
        assert_eq!(receiver.borrow().as_ref().unwrap().sequence, 400);
    }

    #[test]
    fn stalled_renderer_retains_only_latest_snapshot_including_hide() {
        let (publisher, mut receiver) = RendererPublisher::channel();
        for revision in 1..=10000 {
            publisher.publish(RenderSnapshot {
                revision,
                visible: true,
                ..Default::default()
            });
        }
        assert_eq!(
            receiver.borrow_and_update().as_ref().unwrap().revision,
            10000
        );
        publisher.publish(RenderSnapshot {
            revision: 10001,
            visible: false,
            ..Default::default()
        });
        let latest = receiver.borrow_and_update();
        assert_eq!(latest.as_ref().unwrap().revision, 10001);
        assert!(!latest.as_ref().unwrap().visible);
    }

    #[test]
    fn snapshot_preserves_engine_page_and_all_candidates() {
        let response = weasel_common::message::KeyEventResponse {
            page_start: 7,
            can_page_previous: true,
            can_page_next: true,
            selected_candidate: 5,
            candidates: (0..7)
                .map(|i| weasel_common::message::Candidate {
                    text: i.to_string(),
                    comment: String::new(),
                })
                .collect(),
            ..Default::default()
        };
        let snapshot = render_snapshot(
            42,
            3,
            &response,
            &[],
            &RenderRect {
                valid: true,
                ..Default::default()
            },
        );
        assert_eq!(snapshot.items.len(), 7);
        assert_eq!(snapshot.page_start, 7);
        assert_eq!(snapshot.selected_index, 5);
        assert!(snapshot.can_page_previous && snapshot.can_page_next);
        assert_eq!(snapshot.total_item_count, None);
        assert_eq!(snapshot.sequence, 0);
    }

    /// 2026-11-07 上屏后联想：联想条非空且无组字 ⇒ 词进 items（kind=prediction）且可见；
    /// 有候选时联想**不出现**（不能顶掉候选）。
    #[test]
    fn predictions_become_visible_items_only_without_candidates() {
        let response = weasel_common::message::KeyEventResponse {
            commit_text: "你好".into(),
            predictions: vec!["现在".into(), "这个".into()],
            ..Default::default()
        };
        let strip = ["现在".to_owned(), "这个".to_owned()];
        let snapshot = render_snapshot(
            7,
            4,
            &response,
            &strip,
            &RenderRect {
                valid: true,
                ..Default::default()
            },
        );
        assert!(snapshot.visible, "只有联想词时窗口也要显示");
        assert_eq!(snapshot.items.len(), 2);
        assert!(snapshot.items.iter().all(|item| item.kind == "prediction"));
        assert_eq!(snapshot.items[0].primary_text, "现在");
        assert_eq!(snapshot.selected_index, 0, "索引必须落在 items 内");
        assert!(snapshot.preedit.is_none() && !snapshot.can_page_next);

        let with_candidates = weasel_common::message::KeyEventResponse {
            candidates: vec![weasel_common::message::Candidate {
                text: "界面".into(),
                comment: "jiemian".into(),
            }],
            predictions: vec!["现在".into()],
            ..Default::default()
        };
        let snapshot = render_snapshot(
            7,
            5,
            &with_candidates,
            &["现在".to_owned()],
            &RenderRect::default(),
        );
        assert_eq!(snapshot.items.len(), 1, "有候选时不掺联想");
        assert_eq!(snapshot.items[0].kind, "candidate");
    }

    /// 2026-11-08「预测这个标签小一号」的数据面：候选列表里 comment **精确等于**
    /// 「预测」的项标 `kind="prediction"`，其余（拼音注释/「预测词」/带空白的
    /// 「预测」）一律仍是 `"candidate"` —— 否则 theme 会把普通注释也缩小。
    ///
    /// 正负对照（AGENTS.md [m0008]：没有正对照的「通过」是幻觉）：
    ///   * 正：`comment == "预测"` ⇒ prediction；
    ///   * 负：普通拼音注释 / 近义词注释 / 前后带空白的「预测」⇒ candidate。
    #[test]
    fn prediction_candidates_are_tagged_by_exact_comment() {
        use weasel_common::message::Candidate;
        let response = weasel_common::message::KeyEventResponse {
            candidates: vec![
                Candidate {
                    text: "界面".into(),
                    comment: "jie mian".into(),
                },
                Candidate {
                    text: "你好".into(),
                    comment: "预测".into(),
                },
                Candidate {
                    text: "预测".into(),
                    comment: "预测词".into(),
                },
                Candidate {
                    text: "现在".into(),
                    comment: " 预测".into(),
                },
                Candidate {
                    text: "然后".into(),
                    comment: String::new(),
                },
            ],
            ..Default::default()
        };
        let snapshot = render_snapshot(3, 1, &response, &[], &RenderRect::default());
        assert_eq!(snapshot.items.len(), 5);
        assert_eq!(snapshot.items[0].kind, "candidate", "普通拼音注释不是预测项");
        assert_eq!(snapshot.items[1].kind, "prediction", "comment==「预测」应标预测项");
        assert_eq!(snapshot.items[2].kind, "candidate", "「预测词」不是精确「预测」");
        assert_eq!(snapshot.items[3].kind, "candidate", "带前导空白的「预测」不算");
        assert_eq!(snapshot.items[4].kind, "candidate", "空注释是普通候选");
        // 正文文字保持原样（只标 kind，不改文本）。
        assert_eq!(snapshot.items[1].primary_text, "你好");
        assert_eq!(snapshot.items[1].secondary_text, "预测");
    }

    /// 2026-11-08 用户报「联想条关不掉」的回归。
    ///
    /// librime 侧 `Context.predictions` 由 `apply_commit` 填完就没人清，c_api 只要
    /// `context.input.is_empty()` 就照常导出（无组字即成立）。所以上屏之后的**每一帧**
    /// 都带着上一批预测：Esc、方向键、任何不组字的键。联想条是否显示只由引擎的显式
    /// 状态 `strip` 决定 —— 它不是从 `response.predictions` 投影出来的。
    #[test]
    fn stale_predictions_do_not_keep_the_strip_visible() {
        let predictions = || vec!["现在".to_owned(), "这个".to_owned(), "界面".to_owned()];
        // ① 提交那一帧：引擎把预测词填进 strip（见的 `update_prediction_strip`）。
        let commit = weasel_common::message::KeyEventResponse {
            commit_text: "你好".into(),
            predictions: predictions(),
            ..Default::default()
        };
        let shown = render_snapshot(9, 10, &commit, &predictions(), &RenderRect::default());
        assert!(shown.visible, "提交帧要把联想条画出来");
        assert_eq!(shown.items.len(), 3);
        assert!(shown.items.iter().all(|item| item.kind == "prediction"));

        // ② 提交之后的按键（Esc = 0xff1b，无组字、无提交）：引擎照样回旧预测
        //    （脏值没清），但 strip 已被引擎清空 ⇒ 必须给一帧"隐藏"，否则窗口关不掉。
        let after_escape = weasel_common::message::KeyEventResponse {
            predictions: predictions(),
            ..Default::default()
        };
        let hidden = render_snapshot(9, 11, &after_escape, &[], &RenderRect::default());
        assert!(
            !hidden.visible,
            "strip 空时不能把联想条画回来（用户按 Esc 收不掉）"
        );
        assert!(hidden.items.is_empty(), "联想条不得作为 items 留在这帧里");
        assert_eq!(hidden.selected_index, 0);
    }

    /// 2026-11-08 用户报「预测词直接闪一下没了，我怎么选」的回归。
    ///
    /// 提交键（空格）的**抬起帧**紧随提交帧到达引擎：`commit_text` 已被冲成空、
    /// 候选为空、`predictions` 仍是上一批脏值。修复前这一帧把联想条清掉 ⇒ 肉眼
    /// "闪一下"。现在联想条由 strip 显式状态撑着，抬起帧照样可见。
    #[test]
    fn key_release_frame_keeps_the_strip_visible() {
        let strip = ["现在".to_owned(), "这个".to_owned(), "界面".to_owned()];
        // 空格抬起：released 事件，引擎不再提交（commit_text 空），有旧预测脏值。
        let release = weasel_common::message::KeyEventResponse {
            predictions: strip.to_vec(),
            ..Default::default()
        };
        let snapshot = render_snapshot(9, 12, &release, &strip, &RenderRect::default());
        assert!(
            snapshot.visible,
            "抬起帧不得隐藏联想条（否则用户来不及按 1/2/3/4 选词）"
        );
        assert_eq!(snapshot.items.len(), 3);
        assert!(snapshot.items.iter().all(|item| item.kind == "prediction"));
        assert_eq!(snapshot.items[2].primary_text, "界面");
    }
}
