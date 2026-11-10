//! 接收 TSF 按键回调，协调测试/实际按键配对并把可处理事件转发给引擎。
use super::*;
use crate::keyboard;
use weasel_common::message::ContextToken;

/// 缓存已在 `OnTestKey*` 中消费的按键，避免随后 `OnKey*` 重复处理。
pub(super) struct TestedKey {
    /// 上下文与连接标识；任一变化都会使缓存失效。
    ///
    /// **刻意不含 `generation`**：测试回调被引擎接受后，上下文可能因组合取消、
    /// 切换方案等失效操作推高 generation，而物理键并没有变。按 generation 配对
    /// 会把同一个键当成两个键、把实际回调再投递一次（方案选单里数字选完还上屏
    /// 的根因：第二次投递时菜单已关闭，引擎按普通输入处理数字）。
    token: ContextToken,
    /// 虚拟键码（`OnTestKey*`/`OnKey*` 的 wParam）。
    /// 只有键码相同才是同一个物理键，缓存才能吞掉实际回调。
    keycode: u32,
    /// `true` 表示按键释放方向，`false` 表示按下方向。
    key_up: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空格键。
    const SPACE: u32 = 0x20;
    /// 数字键 `1`。
    const DIGIT_1: u32 = 0x31;
    /// 数字键 `2`。
    const DIGIT_2: u32 = 0x32;

    fn token() -> ContextToken {
        ContextToken {
            context_id: 1,
            connection_epoch: 1,
            generation: 1,
        }
    }

    #[test]
    fn win10_test_and_actual_key_process_only_once() {
        let mut pending = None;
        let mut calls = 0;
        // Actual Win10 trace: both repeat count and message time change.
        // Neither is part of callback pairing, deliberately.
        for (test, _lparam, _message_time) in
            [(true, 0x200001u32, 316312), (false, 0x200000, 316359)]
        {
            if !TestedKey::reuse(&mut pending, &token(), false, SPACE, test) {
                calls += 1;
                pending = TestedKey::processed(token(), false, SPACE, test, true);
            }
        }
        assert_eq!(calls, 1);
        assert!(pending.is_none());
    }

    #[test]
    fn test_and_actual_key_pair_across_generation_change() {
        // 测试回调被引擎接受后，上下文可能因组合取消/切换方案推高 generation，
        // 但物理键还是同一个：实际回调必须复用缓存，否则同一个键会被投递两次。
        let bumped_up = ContextToken {
            generation: 2,
            ..token()
        };
        let bumped_down = ContextToken {
            generation: 3,
            ..token()
        };

        let mut pending = TestedKey::processed(token(), false, DIGIT_1, true, true);
        assert!(TestedKey::reuse(&mut pending, &bumped_up, false, DIGIT_1, false));
        assert!(pending.is_none());
        // 缓存已被实际回调消费，不能再吞下一个键。
        assert!(!TestedKey::reuse(&mut pending, &bumped_down, false, DIGIT_1, false));
    }

    #[test]
    fn repeated_tests_reuse_until_actual_callback_for_both_directions() {
        for up in [false, true] {
            let mut pending = TestedKey::processed(token(), up, SPACE, true, true);
            for _ in 0..3 {
                assert!(TestedKey::reuse(&mut pending, &token(), up, SPACE, true));
            }
            assert!(TestedKey::reuse(&mut pending, &token(), up, SPACE, false));
            assert!(pending.is_none());
            assert!(!TestedKey::reuse(&mut pending, &token(), up, SPACE, false));
        }
    }

    #[test]
    fn consecutive_presses_and_autorepeat_each_process_once() {
        let mut pending = None;
        let mut calls = 0;
        for _ in 0..5 {
            for test in [true, false] {
                if !TestedKey::reuse(&mut pending, &token(), false, SPACE, test) {
                    calls += 1;
                    pending = TestedKey::processed(token(), false, SPACE, test, true);
                }
            }
        }
        assert_eq!(calls, 5);
    }

    #[test]
    fn opposite_direction_clears_pending() {
        for up in [false, true] {
            let mut pending = TestedKey::processed(token(), up, SPACE, true, true);
            assert!(!TestedKey::reuse(&mut pending, &token(), !up, SPACE, true));
            assert!(pending.is_none());
        }
    }

    #[test]
    fn different_keycode_does_not_reuse() {
        // 键码不同的实际回调不是同一个物理键：不能吞掉，缓存也要丢弃。
        let mut pending = TestedKey::processed(token(), false, DIGIT_1, true, true);
        assert!(!TestedKey::reuse(&mut pending, &token(), false, DIGIT_2, false));
        assert!(pending.is_none());
        // 测试回调键码不符同样丢弃，避免把下一个键的测试当成重复测试。
        let mut pending = TestedKey::processed(token(), false, DIGIT_1, true, true);
        assert!(!TestedKey::reuse(&mut pending, &token(), false, DIGIT_2, true));
        assert!(pending.is_none());
    }

    #[test]
    fn unhandled_and_actual_keys_do_not_create_pending() {
        assert!(TestedKey::processed(token(), false, SPACE, true, false).is_none());
        assert!(TestedKey::processed(token(), false, SPACE, false, true).is_none());
        assert!(!TestedKey::reuse(&mut None, &token(), false, SPACE, false));
    }

    #[test]
    fn context_and_connection_changes_discard_pending() {
        for changed in [
            ContextToken {
                context_id: 2,
                ..token()
            },
            ContextToken {
                connection_epoch: 2,
                ..token()
            },
            ContextToken {
                connection_epoch: 0,
                ..token()
            },
        ] {
            let mut pending = TestedKey::processed(token(), false, DIGIT_1, true, true);
            assert!(!TestedKey::reuse(&mut pending, &changed, false, DIGIT_1, false));
            assert!(pending.is_none());
        }
    }

    /// 长按退格测试用的上下文 ID。
    const CONTEXT: u64 = 9;

    fn response(eaten: bool, composing: bool) -> KeyEventResponse {
        KeyEventResponse {
            eaten,
            composing,
            ..Default::default()
        }
    }

    #[test]
    fn long_press_swallows_every_repeat_after_code_cleared() {
        let hold = BackspaceHold::default();
        // 短按首帧：编码正好删空，引擎不再组字。
        assert_eq!(hold.before_key(CONTEXT, false), BackspaceAction::Normal);
        hold.after_key(CONTEXT, false, Some(&response(true, false)), true);
        // 之后所有自动重复帧都不再进 RPC，直接吞掉——包括队列里的积压帧。
        for _ in 0..32 {
            assert_eq!(hold.before_key(CONTEXT, true), BackspaceAction::Swallow);
        }
        // 松开后复位：下一次按下重新按短按/引擎响应处理。
        hold.release();
        assert_eq!(hold.before_key(CONTEXT, true), BackspaceAction::Normal);
    }

    #[test]
    fn long_press_forwards_repeats_until_engine_stops_composing() {
        let hold = BackspaceHold::default();
        assert_eq!(hold.before_key(CONTEXT, false), BackspaceAction::Normal);
        hold.after_key(CONTEXT, false, Some(&response(true, true)), true);
        // 编码还有内容：照系统重复节奏逐帧交给引擎删一位，同时由 TIP 保证吞掉。
        assert_eq!(
            hold.before_key(CONTEXT, true),
            BackspaceAction::ForwardAndSwallow
        );
        hold.after_key(CONTEXT, true, Some(&response(true, true)), true);
        assert_eq!(
            hold.before_key(CONTEXT, true),
            BackspaceAction::ForwardAndSwallow
        );
        // 删空的那一帧之后：后续重复帧本地吞掉。
        hold.after_key(CONTEXT, true, Some(&response(true, false)), true);
        assert_eq!(hold.before_key(CONTEXT, true), BackspaceAction::Swallow);
    }

    #[test]
    fn long_press_stops_when_engine_refuses_repeat_with_prefix_left() {
        // 用户边界：raw 编码已空但 preedit 里 committed_prefix 还在。
        // 引擎对这次重复退格返回 eaten=false、composing=true（它拒绝再回退前缀）；
        // TIP 必须就此判定本轮按住耗尽，剩余重复帧本地吞掉，绝不落到宿主。
        let hold = BackspaceHold::default();
        assert_eq!(hold.before_key(CONTEXT, false), BackspaceAction::Normal);
        hold.after_key(CONTEXT, false, Some(&response(true, true)), true);
        assert_eq!(
            hold.before_key(CONTEXT, true),
            BackspaceAction::ForwardAndSwallow
        );
        // 重复帧删空 raw 编码后引擎拒绝：eaten=false && composing=true。
        hold.after_key(CONTEXT, true, Some(&response(false, true)), true);
        for _ in 0..32 {
            assert_eq!(hold.before_key(CONTEXT, true), BackspaceAction::Swallow);
        }
    }

    #[test]
    fn long_press_without_composition_passes_repeats_through() {
        // 英文/无组字状态：首帧引擎不吃，长按的每一帧都照常透传给宿主。
        let hold = BackspaceHold::default();
        assert_eq!(hold.before_key(CONTEXT, false), BackspaceAction::Normal);
        hold.after_key(CONTEXT, false, Some(&response(false, false)), false);
        for _ in 0..16 {
            assert_eq!(hold.before_key(CONTEXT, true), BackspaceAction::Normal);
        }
    }

    #[test]
    fn rpc_timeout_after_armed_keeps_swallowing() {
        // 首帧 RPC 超时：用最近一次响应的组字状态兜底，仍按「组字中」接管。
        let hold = BackspaceHold::default();
        assert_eq!(hold.before_key(CONTEXT, false), BackspaceAction::Normal);
        hold.after_key(CONTEXT, false, None, true);
        assert_eq!(
            hold.before_key(CONTEXT, true),
            BackspaceAction::ForwardAndSwallow
        );
        // 重复帧也超时 ⇒ 停止再发 RPC，剩余帧全部本地吞掉。
        hold.after_key(CONTEXT, true, None, true);
        assert_eq!(hold.before_key(CONTEXT, true), BackspaceAction::Swallow);
    }

    #[test]
    fn rpc_timeout_while_composing_is_swallowed_not_passed_to_host() {
        // 现场回归：连打几个拼音时后端超过预算没回来。旧实现直接 BOOL(0)，
        // 宿主把字母当普通文字插入文档，随后引擎返回的组合串/候选再插进来，
        // 形成「编码汉字编码汉字」；退格又会先删掉组合起点前的已上屏汉字。
        assert_eq!(
            timeout_fallback(BackspaceAction::Normal, true),
            TimeoutFallback::Swallow
        );
    }

    #[test]
    fn rpc_timeout_without_composition_still_falls_back_to_host() {
        // 英文/未组字时保留原有回退：后端不可用也要能继续输入。
        assert_eq!(
            timeout_fallback(BackspaceAction::Normal, false),
            TimeoutFallback::PassThrough
        );
    }

    /// 第六条（抄 msime `DeferredKeyFailurePolicy`）：导航键超时是 soft miss，
    /// 不拆连接；被吃键在组字态吞掉且只缓存一次，实际回调绝不重发。
    #[test]
    fn navigation_timeout_is_swallowed_once_and_never_resent() {
        use crate::rpc_worker::is_candidate_navigation_only_key;
        for vk in [0x09, 0x21, 0x22, 0x26, 0x28] {
            assert!(
                is_candidate_navigation_only_key(vk),
                "VK {vk:#04x} 必须算导航键，否则超时会强拆连接"
            );
        }
        let vk = 0x22; // PageDown
        let mut pending = None;
        let mut dispatches = 0;
        for test in [true, false] {
            if !TestedKey::reuse(&mut pending, &token(), false, vk, test) {
                dispatches += 1;
                // 组字态超时：TIP 吞键并缓存为 eaten=true。
                pending = TestedKey::processed(token(), false, vk, test, true);
            }
        }
        assert_eq!(
            dispatches, 1,
            "测试回调 + 实际回调只能进一次 RPC，被吃键绝不重发"
        );
        assert!(pending.is_none());
        assert_eq!(
            timeout_fallback(BackspaceAction::Normal, true),
            TimeoutFallback::Swallow,
            "组字态导航键超时必须吞掉，不能透传给宿主翻页"
        );
    }

    #[test]
    fn short_backspace_timeout_uses_composing_hint() {
        // 短按退格没有长按接管状态，仍然要按最近一次组字状态吞掉：
        // 否则透传的退格会删到 StartComposition 插入点前面的已上屏汉字。
        let hold = BackspaceHold::default();
        assert_eq!(hold.before_key(CONTEXT, false), BackspaceAction::Normal);
        hold.after_key(CONTEXT, false, None, true);
        assert_eq!(
            timeout_fallback(BackspaceAction::Normal, true),
            TimeoutFallback::Swallow
        );
    }

    #[test]
    fn rpc_timeout_on_armed_backspace_repeat_is_swallowed() {
        // 已接管的长按重复帧即使最近一次响应缺失组字提示，也必须继续吞。
        assert_eq!(
            timeout_fallback(BackspaceAction::ForwardAndSwallow, false),
            TimeoutFallback::Swallow
        );
    }

    #[test]
    fn response_from_connection_established_in_request_still_belongs() {
        // 断连后首个按键：请求时 epoch=0，worker 在请求内建连并把 token 填成
        // 新 epoch。旧实现整份比较 → 判失配 → BOOL(0)，这一键的字母漏进文档。
        let baseline = ContextToken {
            context_id: 1,
            connection_epoch: 0,
            generation: 4,
        };
        let attached = ContextToken {
            connection_epoch: 9,
            ..baseline.clone()
        };
        assert!(response_matches_request(Some(&attached), &baseline));
    }

    #[test]
    fn response_from_migrated_context_is_still_rejected() {
        let baseline = ContextToken {
            context_id: 1,
            connection_epoch: 3,
            generation: 4,
        };
        // 已有连接时 epoch 变化 = 上下文被迁移，不能接受。
        assert!(!response_matches_request(
            Some(&ContextToken {
                connection_epoch: 5,
                ..baseline.clone()
            }),
            &baseline
        ));
        assert!(!response_matches_request(
            Some(&ContextToken {
                context_id: 2,
                ..baseline.clone()
            }),
            &baseline
        ));
        assert!(!response_matches_request(
            Some(&ContextToken {
                generation: 5,
                ..baseline.clone()
            }),
            &baseline
        ));
        assert!(!response_matches_request(None, &baseline));
    }

    #[test]
    fn context_switch_does_not_inherit_swallow() {
        let hold = BackspaceHold::default();
        assert_eq!(hold.before_key(CONTEXT, false), BackspaceAction::Normal);
        hold.after_key(CONTEXT, false, Some(&response(true, false)), true);
        assert_eq!(hold.before_key(CONTEXT, true), BackspaceAction::Swallow);
        // 换到下个上下文：不继承上一段按住的吞键状态，按正常按键处理。
        assert_eq!(hold.before_key(CONTEXT + 1, true), BackspaceAction::Normal);
    }
}

impl TestedKey {
    /// 判断当前 TSF 回调是否对应已处理的测试回调。
    ///
    /// 配对应依据键码、方向和上下文/连接标识，而非 Windows 消息时间、重复计数
    /// 或上下文代次。重复测试回调保留缓存，实际按键回调消费缓存；键码、方向或
    /// 上下文/连接不符时丢弃旧值。
    fn reuse(
        pending: &mut Option<Self>,
        token: &ContextToken,
        key_up: bool,
        keycode: u32,
        test: bool,
    ) -> bool {
        let Some(previous) = pending.take() else {
            return false;
        };
        if previous.token.context_id != token.context_id
            || previous.token.connection_epoch != token.connection_epoch
            || token.connection_epoch == 0
            || previous.key_up != key_up
            || previous.keycode != keycode
        {
            return false;
        }
        if test {
            *pending = Some(previous);
        }
        true
    }

    /// 仅为已消费的测试回调建立缓存；实际回调、未消费事件和失效连接不缓存。
    fn processed(
        token: ContextToken,
        key_up: bool,
        keycode: u32,
        test: bool,
        eaten: bool,
    ) -> Option<Self> {
        (test && eaten && token.connection_epoch != 0).then_some(Self {
            token,
            keycode,
            key_up,
        })
    }
}

/// 长按退格的重复帧策略。
///
/// Windows 按住键产生的自动重复帧（`lParam` bit 30）会逐帧进入 TSF 回调。
/// 首帧不带重复位，完全沿用引擎响应（短按行为不变）；按住期间的后续帧只有在
/// 「首帧时引擎仍在组字」的前提下才由 TIP 负责吞掉：编码未删空时照常转发给引擎
/// 删一位，引擎报告不再组字后不再发 RPC、直接返回已消费。
///
/// 这样既跟随系统重复节奏删编码，也保证清空编码后的重复帧不会落到宿主编辑区。
#[derive(Default)]
pub(super) struct BackspaceHold {
    /// 本次按住是否以「引擎在组字」开始；只有这种按住才吞重复帧。
    armed: AtomicBool,
    /// 引擎已报告编码清空；剩余重复帧本地吞掉，不再进入 RPC 队列。
    exhausted: AtomicBool,
    /// 状态所属上下文；跨上下文不继承吞键行为。
    context_id: AtomicU64,
}

/// 单个退格帧的处置方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackspaceAction {
    /// 不拦截：按引擎响应决定是否消费。短按、英文/无组字长按都走这里。
    Normal,
    /// 转发给引擎删一位编码，但这一帧无论引擎怎么应答都由 TIP 返回已消费。
    ForwardAndSwallow,
    /// 编码已清空：不再发 RPC，直接返回已消费。
    Swallow,
}

/// RPC 在宿主等待预算内没有返回时，这一键的归属。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimeoutFallback {
    /// 交还宿主，按普通按键处理（英文、标点、未组字时的正常回退）。
    PassThrough,
    /// 由 TIP 吞掉：组字态下透传会让宿主把编码当普通文字插进文档，
    /// 与引擎里的组合串错位（「编码汉字编码汉字」、退格先删已上屏汉字）。
    Swallow,
}

/// 响应是否属于本次请求。
///
/// `rpc_worker` 在请求内才建立连接时会把事件令牌的 `connection_epoch` 填成新值，
/// 因此响应代次与请求快照不同并不代表上下文被迁移；上下文身份与 generation 才是
/// 必须一致的字段。旧实现按整份令牌比较，导致「断连后首个按键」即使被引擎吃下
/// 也会退回宿主，字母漏进文档。
fn response_matches_request(
    response: Option<&ContextToken>,
    baseline: &ContextToken,
) -> bool {
    let Some(response) = response else {
        return false;
    };
    response.context_id == baseline.context_id
        && response.generation == baseline.generation
        && (response.connection_epoch == baseline.connection_epoch
            || baseline.connection_epoch == 0)
}

/// 超时后的按键归属判定：只要引擎可能仍在组字，就必须吞掉。
///
/// `composing_hint` 是最近一次成功响应的组字状态；长按退格的重复帧即便没有
/// 组字提示，也已由 [`BackspaceHold`] 接管，必须继续吞。
fn timeout_fallback(action: BackspaceAction, composing_hint: bool) -> TimeoutFallback {
    if action == BackspaceAction::ForwardAndSwallow || composing_hint {
        TimeoutFallback::Swallow
    } else {
        TimeoutFallback::PassThrough
    }
}

impl BackspaceHold {
    /// 按键进入引擎前的判定；同时负责在上下文切换/新一次按下时复位状态。
    fn before_key(&self, context_id: u64, repeat: bool) -> BackspaceAction {
        if self.context_id.load(Ordering::Acquire) != context_id {
            self.context_id.store(context_id, Ordering::Release);
            self.release();
        }
        if !repeat {
            // 每次新的按下都重新判定，避免把上一次按住的吞键状态带进来。
            self.release();
            return BackspaceAction::Normal;
        }
        if self.exhausted.load(Ordering::Acquire) {
            BackspaceAction::Swallow
        } else if self.armed.load(Ordering::Acquire) {
            BackspaceAction::ForwardAndSwallow
        } else {
            BackspaceAction::Normal
        }
    }

    /// 引擎响应回来后的状态更新；`composing_hint` 是最近一次响应的组字状态，
    /// 仅在本次 RPC 不可用时兜底。
    fn after_key(
        &self,
        context_id: u64,
        repeat: bool,
        response: Option<&KeyEventResponse>,
        composing_hint: bool,
    ) {
        if self.context_id.load(Ordering::Acquire) != context_id {
            return;
        }
        if repeat {
            // 引擎不再接受这个重复退格（raw 编码已空、已停止组字，或 RPC 超时）
            // ⇒ 本轮按住后续帧全部本地吞掉，绝不再落到宿主。
            if self.armed.load(Ordering::Acquire)
                && !response.is_some_and(|response| response.eaten && response.composing)
            {
                self.exhausted.store(true, Ordering::Release);
            }
            return;
        }
        let (armed, exhausted) = match response {
            Some(response) => (response.eaten, response.eaten && !response.composing),
            // 引擎不可用：用最近一次已知组字状态兜底，避免整段长按直接透传。
            None => (composing_hint, !composing_hint),
        };
        self.armed.store(armed, Ordering::Release);
        self.exhausted.store(exhausted, Ordering::Release);
    }

    /// 松开退格（或丢失按键）后复位。
    fn release(&self) {
        self.armed.store(false, Ordering::Release);
        self.exhausted.store(false, Ordering::Release);
    }
}

impl TextService {
    /// 处理 TSF 按键回调并返回是否由输入法消费。
    ///
    /// 该入口运行在宿主 COM/TSF 回调边界：不可等待重入锁；只读或安全输入上下文、
    /// 无效服务状态及不可用引擎均应让宿主继续处理。响应在应用前须与上下文令牌匹配，
    /// 并先排空同一有序 RPC 流上的更新，避免覆盖较早到达的渲染提交。
    pub(super) fn forward_key_event(
        &self,
        context: Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
        key_up: bool,
        test: bool,
        session: IUnknown,
    ) -> Result<BOOL> {
        // 乙-1 插桩：整段 TSF 回调的耗时（含 TSF 状态查询 + RPC 等待 + 应用结果），
        // 这是"应用 UI 线程被输入法占住多久"的完整口径。
        let probe = crate::key_latency::probe();
        let result = self.forward_key_event_inner(context, wparam, lparam, key_up, test, session);
        probe.finish(crate::key_latency::Outcome::Total, matches!(result, Ok(BOOL(1))));
        result
    }

    /// [`forward_key_event`] 的本体，外层只负责插桩。
    fn forward_key_event_inner(
        &self,
        context: Ref<'_, ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
        key_up: bool,
        test: bool,
        session: IUnknown,
    ) -> Result<BOOL> {
        // 松开退格即结束本次长按；即使随后因上下文状态提前返回也要复位。
        if key_up && wparam.0 as u32 == keyboard::VK_BACK {
            self.backspace_hold.release();
        }
        if !self.activated.load(Ordering::Acquire) {
            self.lock(&self.tested_key)?.take();
            return Ok(BOOL(0));
        }
        // Let the shell see our explicit Win+. action, not the Rime engine.
        if keyboard::is_emoji_shortcut() {
            self.lock(&self.tested_key)?.take();
            return Ok(BOOL(0));
        }
        let Some(context) = context.to_owned() else {
            self.lock(&self.tested_key)?.take();
            return Ok(BOOL(0));
        };
        // A document context can represent a non-editable part of a web page.
        // Query on each callback: the same context may later become writable.
        let writable = context_is_writable(unsafe { context.GetStatus() });
        if !writable {
            self.lock(&self.tested_key)?.take();
            return Ok(BOOL(0));
        }
        let state = self.ensure_context(context, &session)?;
        if !state.alive.load(Ordering::Acquire) || state.suspended.load(Ordering::Acquire) {
            self.lock(&self.tested_key)?.take();
            return Ok(BOOL(0));
        }
        self.focus_context(Some(state.clone()))?;
        if state.finishing_raw.load(Ordering::Acquire) {
            self.lock(&self.tested_key)?.take();
            return Ok(BOOL(0));
        }
        if state.secure_field.load(Ordering::Acquire) == secure_input::UNKNOWN {
            // Resolve the first key synchronously. If the host refuses a read
            // session, fail closed for this key and retry on the next callback.
            let _ = self.request_secure_field_probe(&state, session.clone(), true);
        }
        if self.should_bypass_secure_field(&state)? {
            self.lock(&self.tested_key)?.take();
            self.reconcile_secure_field()?;
            self.refresh_language_bar()?;
            return Ok(BOOL(0));
        }
        if self.cleanup_disconnected_composition(&state, &session)? {
            return Ok(BOOL(0));
        }
        let token = state.token()?;
        // 请求时的 token 快照：下面 drain_context_updates 处理切方案的 context
        // 更新时可能推高 generation，:248 必须用这份快照核对响应，不能重新取
        // 变化后的 state.token()，否则会把本该被吞掉的数字键误判失配退回上屏。
        let baseline = token.clone();
        // 长按退格：重复帧（lParam bit30）逐帧到达。只有「首帧时引擎在组字」的
        // 按住才由 TIP 接管后续重复帧，短按与英文/无组字状态仍完全按引擎响应走。
        let backspace = !key_up && wparam.0 as u32 == keyboard::VK_BACK;
        let repeat = backspace && keyboard::is_autorepeat(lparam.0 as i64);
        match self.lock(&self.tested_key) {
            Ok(mut cached) => {
                if TestedKey::reuse(&mut cached, &token, key_up, wparam.0 as u32, test) {
                    // 真实回调复用测试回调的结果：此时绝不能再次重置/推进按住状态，
                    // 否则首帧刚 arm 的 armed 会被这里清掉，长按又退回透传。
                    return Ok(BOOL(1));
                }
            }
            Err(error) => {
                // 重入导致缓存锁不可用：先按当前状态判定，已接管的重复帧仍要吞掉。
                if backspace
                    && self.backspace_hold.before_key(state.id, repeat) != BackspaceAction::Normal
                {
                    crate::key_latency::swallowed();
                    return Ok(BOOL(1));
                }
                return Err(error);
            }
        }
        let action = if backspace {
            self.backspace_hold.before_key(state.id, repeat)
        } else {
            BackspaceAction::Normal
        };
        if action == BackspaceAction::Swallow {
            // 编码已清空：这一帧不再进入 RPC，直接吞掉。缓存锁不可用也无碍——
            // 随后的实际回调会重新判定 exhausted 并同样吞掉。
            crate::key_latency::swallowed();
            if let Ok(mut cached) = self.lock(&self.tested_key) {
                *cached = TestedKey::processed(baseline.clone(), key_up, wparam.0 as u32, test, true);
            }
            return Ok(BOOL(1));
        }
        let Some(mut event) = keyboard::translate(wparam.0 as u32, lparam.0 as i64, key_up) else {
            // 退格总能翻译成 0xff08；接管中的重复帧即使翻译失败也不退回宿主。
            return Ok(BOOL(if action == BackspaceAction::ForwardAndSwallow {
                1
            } else {
                0
            }));
        };
        event.token = Some(token);
        let response = match self.lock(&state.rpc) {
            Ok(mut rpc) => rpc.process_key_event(event),
            Err(error) => {
                // 重入导致 RPC 锁不可用：已接管的长按重复帧仍必须吞掉。
                if action == BackspaceAction::ForwardAndSwallow {
                    crate::key_latency::swallowed();
                    if let Ok(mut cached) = self.lock(&self.tested_key) {
                        *cached = TestedKey::processed(
                            baseline.clone(),
                            key_up,
                            wparam.0 as u32,
                            test,
                            true,
                        );
                    }
                    return Ok(BOOL(1));
                }
                return Err(error);
            }
        };
        // 最近一次成功响应的组字状态：RPC 超时/队列满时用它决定这一键的归属。
        let composing_hint = state.composing.load(Ordering::Acquire);
        if backspace {
            self.backspace_hold
                .after_key(state.id, repeat, response.as_ref(), composing_hint);
        }
        let Some(response) = response else {
            if timeout_fallback(action, composing_hint) == TimeoutFallback::Swallow {
                // 组字态下后端超时：绝不能把键透传宿主。宿主会把字母当普通文字插进
                // 文档，而引擎里的组合串仍在推进，于是出现「编码汉字编码汉字」、
                // 退格先删掉组合起点前的已上屏汉字。普通键的连接已被 rpc_worker
                // 置为失效，下一键重建连接后由引擎返回完整组合串重新同步；
                // 导航键（Tab/PageUp/PageDown/Up/Down）是 soft miss，连接保留，
                // 但这一键同样丢掉即止，绝不重发。
                crate::key_latency::swallowed();
                if let Ok(mut cached) = self.lock(&self.tested_key) {
                    *cached = TestedKey::processed(
                        baseline.clone(),
                        key_up,
                        wparam.0 as u32,
                        test,
                        true,
                    );
                }
                return Ok(BOOL(1));
            }
            // 无组字（英文/标点/空编码）：后端不可用时让宿主按普通按键处理，
            // 下一键再重建连接。
            self.refresh_language_bar()?;
            return Ok(BOOL(0));
        };
        if action == BackspaceAction::ForwardAndSwallow {
            // 已决定吞掉这一帧：即使响应 token 失配或上下文更新失败，也不把退格
            // 退回宿主；响应仍要排空，避免编码栏漏刷新。
            if response_matches_request(response.token.as_ref(), &baseline) {
                state.composing.store(response.composing, Ordering::Release);
            }
            let _ = self.drain_context_updates(&session);
            crate::key_latency::swallowed();
            if let Ok(mut cached) = self.lock(&self.tested_key) {
                *cached = TestedKey::processed(baseline.clone(), key_up, wparam.0 as u32, test, true);
            }
            return Ok(BOOL(1));
        }
        let eaten = response.eaten;
        // Connection may have been established during this request. Use the
        // response epoch, and never retain a result from a superseded session.
        if let Some(token) = response.token.as_ref() {
            if state.matches(Some(token))? {
                state.composing.store(response.composing, Ordering::Release);
                *self.lock(&self.tested_key)? = TestedKey::processed(
                    token.clone(),
                    key_up,
                    wparam.0 as u32,
                    test,
                    eaten,
                );
            }
        }

        // Responses, including this one, are delivered on one ordered stream.
        // Do not apply a direct reply ahead of a queued renderer commit.
        self.drain_context_updates(&session)?;
        // 用请求时的 token 快照复核：这里只确认上下文身份/代次没被迁移或失效。
        // 不能用 state.token()——drain 之后 generation 可能已被切方案推高，旧响应
        // 会因此被误判失配而退回上屏；请求内才建立连接时 epoch 变化则应放行。
        if !response_matches_request(response.token.as_ref(), &baseline) {
            return Ok(BOOL(0));
        }
        Ok(BOOL(eaten as i32))
    }
}

/// 将 TSF 状态查询解释为可编辑性；查询失败时采取放行按键的保守策略。
pub(super) fn context_is_writable(status: Result<bindings::TF_STATUS>) -> bool {
    status
        .map(|status| status.dwDynamicFlags & bindings::TF_SD_READONLY == 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod readonly_tests {
    use super::*;

    #[test]
    fn readonly_and_unavailable_contexts_pass_keys_through() {
        assert!(!context_is_writable(Ok(bindings::TF_STATUS {
            dwDynamicFlags: bindings::TF_SD_READONLY,
            ..Default::default()
        })));
        assert!(!context_is_writable(Err(Error::from_hresult(
            boundary::E_FAIL
        ))));
        // No cached disable flag: a subsequent writable status allows input.
        assert!(context_is_writable(Ok(bindings::TF_STATUS::default())));
    }
}
