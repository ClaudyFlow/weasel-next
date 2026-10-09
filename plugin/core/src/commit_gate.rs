//! 上屏提交的**准入判定**（对应 Lua 版 commit 回调开头的四道守卫）。
//!
//! Lua 原版（`input_statistics.lua` L1210-1229）在统计一次上屏之前会做如下判定：
//!
//! ```lua
//! local returnFlg = 0
//! if not commit_text or commit_text == "" then returnFlg = 1 end      -- ① 空上屏
//! if ctx.input == commit_text then returnFlg = 1 end                  -- ② 编码与上屏一致
//! if ctx.input:find("^/") then returnFlg = 1 end                      -- ③ 命令输入
//! if not isAllChineseCharacter(commit_text) then returnFlg = 1 end    -- ④ 上屏含非汉字
//! if 1 == returnFlg then keyTouchCnt = 0; return end
//! if commit_text:match("^[！!@#$％^&?,.;？，。；/0123456789]+$") then return end -- ⑤ 纯标点/数字
//! ```
//!
//! 前四条一旦命中，本次上屏**不统计，并且把击键计数清零**；第⑤条同样不统计，但**不清零**
//! （Lua 里它是 `return` 而不是 `keyTouchCnt = 0; return`）。
//!
//! 漏掉这些守卫的后果很直观：上屏统计报表（`/rtj` 产生的那段 1000+ 字符的文本）时，会被
//! 当成一次"输入了 1000 多字"的上屏，字数、均速、平均码长全部失真。

/// 一次上屏的处理方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitDisposition {
    /// 正常统计。
    Count,
    /// 不统计，并清零击键计数（Lua 的 `returnFlg = 1` 四种情形）。
    SkipAndResetKeys,
    /// 不统计，但保留击键计数（Lua 的纯标点/数字分支）。
    SkipKeepKeys,
}

/// 判断码点是否为 Lua 版认可的"汉字"。
///
/// 区间逐条照抄 Lua 的 `IsChineseCharacter`（规范 §8）：基本区、扩展 A–I、笔画、部首、
/// 康熙部首、兼容区、兼容补充、汉字结构、注音与注音扩展。**闭区间**；不含全角标点、
/// 不含日文假名、不含 `0x3000–0x303F` 标点。
pub fn is_chinese_char(code_point: u32) -> bool {
    matches!(code_point,
        0x4E00..=0x9FFF      // CJK 统一表意文字（基本区）
        | 0x3400..=0x4DBF    // 扩展 A
        | 0x20000..=0x2A6DF  // 扩展 B
        | 0x2A700..=0x2B73F  // 扩展 C
        | 0x2B740..=0x2B81F  // 扩展 D
        | 0x2B820..=0x2CEAF  // 扩展 E
        | 0x2CEB0..=0x2EBEF  // 扩展 F
        | 0x30000..=0x3134F  // 扩展 G
        | 0x31350..=0x323AF  // 扩展 H
        | 0x2EBF0..=0x2EE5F  // 扩展 I
        | 0x31C0..=0x31EF    // CJK 笔画
        | 0x2E80..=0x2EFF    // CJK 部首扩展
        | 0x2F00..=0x2FDF    // 康熙部首
        | 0xF900..=0xFADF    // CJK 兼容表意文字
        | 0x2F800..=0x2FA1F  // CJK 兼容表意文字补充
        | 0x2FF0..=0x2FFF    // 汉字结构描述符
        | 0x3100..=0x312F    // 注音符号
        | 0x31A0..=0x31BF    // 注音符号扩展
    )
}

/// 是否整段都是汉字；**空串按 Lua 语义返回 `true`**（循环不执行）。
pub fn is_all_chinese(text: &str) -> bool {
    text.chars().all(|c| is_chinese_char(c as u32))
}

/// 判定一次上屏是否计入统计（对应 Lua commit 回调开头的守卫序列）。
///
/// `input` 是提交时的编码串（Lua 的 `ctx.input`）。
pub fn classify_commit(input: &str, commit_text: &str) -> CommitDisposition {
    // ① 空上屏
    if commit_text.is_empty() {
        return CommitDisposition::SkipAndResetKeys;
    }
    // ② 编码与上屏一致（例如编码直接上屏）
    if input == commit_text {
        return CommitDisposition::SkipAndResetKeys;
    }
    // ③ 命令输入（以 / 引导）
    if input.starts_with('/') {
        return CommitDisposition::SkipAndResetKeys;
    }
    // ④ 上屏文本含非汉字
    if !is_all_chinese(commit_text) {
        return CommitDisposition::SkipAndResetKeys;
    }
    // ⑤ 纯标点/数字（含半角与全角、以及 Lua 原文列出的符号集）
    if is_pure_punctuation_or_digits(commit_text) {
        return CommitDisposition::SkipKeepKeys;
    }
    CommitDisposition::Count
}

/// Lua 的 `commit_text:match("^[！!@#$％^&?,.;？，。；/0123456789]+$")` 等价实现。
fn is_pure_punctuation_or_digits(text: &str) -> bool {
    const PUNCTUATION: &str = "！!@#$％^&?,.;？，。；/0123456789";
    !text.is_empty() && text.chars().all(|c| PUNCTUATION.contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_ranges_match_lua() {
        // 基本区与常见扩展
        assert!(is_chinese_char(0x4E00)); // 一
        assert!(is_chinese_char(0x9FFF));
        assert!(is_chinese_char(0x3400));
        assert!(is_chinese_char(0x20000));
        assert!(is_chinese_char(0x2EBF0));
        assert!(is_chinese_char(0xF900));
        assert!(is_chinese_char(0x3100));
        // 不属于：ASCII、全角标点、假名、注音之外的空格
        assert!(!is_chinese_char('a' as u32));
        assert!(!is_chinese_char('1' as u32));
        assert!(!is_chinese_char('，' as u32)); // U+FF0C 全角逗号
        assert!(!is_chinese_char('。' as u32)); // U+3002
        assert!(!is_chinese_char('※' as u32)); // 报表里的符号
        assert!(!is_chinese_char(0x3042)); // あ
        assert!(!is_chinese_char(0x20)); // 空格
    }

    #[test]
    fn all_chinese_matches_lua_including_empty() {
        assert!(is_all_chinese("你好世界"));
        assert!(is_all_chinese(""));
        assert!(!is_all_chinese("你好，世界"), "含全角逗号即非全汉字");
        assert!(!is_all_chinese("hello"));
        assert!(!is_all_chinese("你好world"));
    }

    #[test]
    fn empty_commit_is_skipped_and_resets_keys() {
        assert_eq!(classify_commit("", ""), CommitDisposition::SkipAndResetKeys);
    }

    #[test]
    fn input_equal_to_text_is_skipped() {
        // 编码直接上屏：ctx.input == commit_text
        assert_eq!(
            classify_commit("hello", "hello"),
            CommitDisposition::SkipAndResetKeys
        );
    }

    #[test]
    fn command_input_is_never_counted() {
        // 这正是统计报表被算进"输入 1138 字"的原因：/rtj 上屏的报表必须跳过。
        let report = "上屏 14 次，输入 1138 字\n极速 2302.5，击键0.7，码长0.0";
        assert_eq!(
            classify_commit("/rtj", report),
            CommitDisposition::SkipAndResetKeys
        );
        assert_eq!(
            classify_commit("/pf", "※ 可用皮肤列表："),
            CommitDisposition::SkipAndResetKeys
        );
    }

    #[test]
    fn non_chinese_commit_is_skipped() {
        // 上屏英文、数字、含标点的文本都不统计（Lua 的 isAllChineseCharacter 守卫）。
        assert_eq!(
            classify_commit("nihao", "hello"),
            CommitDisposition::SkipAndResetKeys
        );
        assert_eq!(
            classify_commit("nihao", "你好，"),
            CommitDisposition::SkipAndResetKeys
        );
        assert_eq!(
            classify_commit("yi", "1234"),
            CommitDisposition::SkipAndResetKeys
        );
    }

    #[test]
    fn chinese_commit_is_counted() {
        assert_eq!(classify_commit("nihao", "你好"), CommitDisposition::Count);
        assert_eq!(classify_commit("nh", "你好世界"), CommitDisposition::Count);
    }

    #[test]
    fn punctuation_is_caught_by_the_chinese_guard_before_the_punctuation_branch() {
        // Lua 的第 ⑤ 条（纯标点/数字 → 仅 return）实际上**永远走不到**：标点都不是汉字，
        // 会先被第 ④ 条 `isAllChineseCharacter` 拦下，于是走的是"清零击键数再 return"分支。
        // 这里如实复现该行为，并把 ⑤ 对应的枚举值保留为"文档化的死代码"。
        for (input, text) in [
            ("dian", "。"),
            ("dou", "，"),
            ("q", "?"),
            ("n", "123"),
            ("x", "！@#"),
        ] {
            assert_eq!(
                classify_commit(input, text),
                CommitDisposition::SkipAndResetKeys,
                "输入 {input} / 上屏 {text} 应先被第 ④ 条拦下"
            );
        }
    }

    #[test]
    fn skip_keep_keys_variant_is_documented_dead_code() {
        // 只有当某串"全部由汉字区间内的字符组成"且同时属于 Lua 第 ⑤ 条字符集时才会走到
        // SkipKeepKeys——这两个集合没有交集，因此该分支不可达。测试固化了这一判断，
        // 以免将来有人误以为需要额外处理。
        const PUNCTUATION: &str = "！!@#$％^&?,.;？，。；/0123456789";
        for c in PUNCTUATION.chars() {
            assert!(
                !is_chinese_char(c as u32),
                "{c} 若变成汉字区间成员，第 ⑤ 条就不再是死代码"
            );
        }
    }
}
