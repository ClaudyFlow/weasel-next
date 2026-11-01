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
        {
            let mut cached = self.lock(&self.tested_key)?;
            if TestedKey::reuse(&mut cached, &token, key_up, wparam.0 as u32, test) {
                return Ok(BOOL(1));
            }
        }
        let Some(mut event) = keyboard::translate(wparam.0 as u32, lparam.0 as i64, key_up) else {
            return Ok(BOOL(0));
        };
        event.token = Some(token);
        let response = self.lock(&state.rpc)?.process_key_event(event);
        let Some(response) = response else {
            // The out-of-process server is optional during activation.  Let
            // the host handle the key as ordinary text and retry connection
            // on the next input event.
            self.refresh_language_bar()?;
            return Ok(BOOL(0));
        };
        let eaten = response.eaten;
        // Connection may have been established during this request. Use the
        // response epoch, and never retain a result from a superseded session.
        if let Some(token) = response.token.as_ref() {
            if state.matches(Some(token))? {
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
        // 用请求时的 token 快照复核：rpc_worker 已保证响应 token 与请求一致，
        // 这里只确认上下文身份没被迁移/失效。不能用 state.token()——drain 之后
        // generation 可能已被切方案推高，旧响应会因此被误判失配而退回上屏。
        if response.token.as_ref() != Some(&baseline) {
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
