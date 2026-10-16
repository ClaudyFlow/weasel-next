//! 官方 weasel 的颜色语义（`WeaselStyleColor.h` 的 `ParseColorValue` +
//! `RimeWithWeasel.cpp` 的 `_UpdateUIStyleColor`）。
//!
//! 官方规则（逐条对齐，别凭直觉改）：
//!
//! 1. 文本形式：`#RGB` / `#ARGB` / `#RRGGBB` / `#AARRGGBB`，`0x` 前缀等价于 `#`，
//!    也可以是十进制整数。
//! 2. **位数决定有没有 alpha**：3/6 位不带 alpha（一律按不透明），4/8 位才带 alpha。
//!    所以 `0x000000` 是**不透明黑**，而 `0x00000000` 是**全透明**——两者完全不同。
//! 3. 通道顺序由配色的 `color_format` 决定，**默认是 `abgr`**（官方
//!    `ColorFormat fmt = COLOR_ABGR`），另有 `argb`、`rgba`。也就是说官方皮肤文件里
//!    `text_color: 0xffe8ca` 的 R 在最低字节，实际是 RGB(0xca,0xe8,0xff)——浅蓝。
//! 4. 我们内部统一用 **ARGB `0xAARRGGBB`**（设置界面的色块、D2D 取通道都按这个），
//!    因此在解析时就把官方的字节序翻译过来了。

/// 官方 `color_format`：颜色文本的通道顺序。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColorFormat {
    /// `0xAABBGGRR`（官方默认）。
    #[default]
    Abgr,
    /// `0xAARRGGBB`。
    Argb,
    /// `0xRRGGBBAA`。
    Rgba,
}

impl ColorFormat {
    /// 按官方 `_RimeParseStringOptWithFallback` 的名字解析，未知一律回落到 `abgr`。
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "argb" => Self::Argb,
            "rgba" => Self::Rgba,
            _ => Self::Abgr,
        }
    }

    /// 写回官方配置时使用的名字。
    pub fn name(self) -> &'static str {
        match self {
            Self::Abgr => "abgr",
            Self::Argb => "argb",
            Self::Rgba => "rgba",
        }
    }
}

/// 把一个十六进制串按官方规则展开成 `(值, 是否带 alpha)`。
///
/// 官方对 3 位/4 位短形式先逐位复制成 6 位/8 位，再按位数决定 alpha：
/// 3→6 位不带 alpha，4→8 位带 alpha。
fn expand_hex(digits: &str) -> Option<(u32, bool)> {
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (text, short): (String, bool) = match digits.len() {
        3 => (
            digits.chars().flat_map(|c| [c, c]).collect::<String>(),
            true,
        ),
        4 => (
            digits.chars().flat_map(|c| [c, c]).collect::<String>(),
            false,
        ),
        6 => (digits.to_string(), true),
        8 => (digits.to_string(), false),
        _ => return None,
    };
    u32::from_str_radix(&text, 16).ok().map(|value| (value, short))
}

/// 按官方语义解析颜色，返回内部的 ARGB `0xAARRGGBB`。
///
/// 无法解析时返回 `None`（官方此时保留 fallback 值，调用方决定回落）。
pub fn parse(text: &str, format: ColorFormat) -> Option<u32> {
    let text = text.trim().trim_matches('"').trim_matches('\'');
    if text.is_empty() {
        return None;
    }
    let (value, short) = match text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .or_else(|| text.strip_prefix('#'))
    {
        Some(digits) => expand_hex(digits)?,
        None => {
            if !text.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let value: u32 = text.parse().ok()?;
            // 十进制：不超过 0xffffff 视为"没写 alpha"（官方同款判断）
            (value, value <= 0x00ff_ffff)
        }
    };
    let a = if short { 0xff } else { (value >> 24) & 0xff };
    let (r, g, b) = match format {
        ColorFormat::Abgr => (value & 0xff, (value >> 8) & 0xff, (value >> 16) & 0xff),
        ColorFormat::Argb => ((value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff),
        ColorFormat::Rgba => ((value >> 24) & 0xff, (value >> 16) & 0xff, (value >> 8) & 0xff),
    };
    Some((a << 24) | (r << 16) | (g << 8) | b)
}

/// 把内部 ARGB 写回官方风格的文本：不透明时用 6 位（不带 alpha），否则用 8 位。
///
/// 生成 `default.theme.yaml` 时用它——这样"不透明黑"与"全透明"不会在往返中互相污染。
pub fn format_text(argb: u32, format: ColorFormat) -> String {
    let a = (argb >> 24) & 0xff;
    let r = (argb >> 16) & 0xff;
    let g = (argb >> 8) & 0xff;
    let b = argb & 0xff;
    let value = match format {
        ColorFormat::Abgr => (r) | (g << 8) | (b << 16),
        ColorFormat::Argb => (r << 16) | (g << 8) | b,
        ColorFormat::Rgba => (r << 24) | (g << 16) | (b << 8),
    };
    if a == 0xff {
        format!("0x{value:06x}")
    } else {
        let value = match format {
            ColorFormat::Abgr | ColorFormat::Argb => (a << 24) | value,
            ColorFormat::Rgba => value | a,
        };
        format!("0x{value:08x}")
    }
}

/// 官方 `blend_colors`：把前景按自身 alpha 混到背景上，返回不透明/半透明结果。
///
/// 入参与返回都是内部 ARGB。完全透明的前景按官方做法直接返回背景。
pub fn blend(foreground: u32, background: u32) -> u32 {
    let channel = |color: u32, shift: u32| ((color >> shift) & 0xff) as f32;
    let f_alpha = channel(foreground, 24) / 255.0;
    let b_alpha = channel(background, 24) / 255.0;
    let out_alpha = f_alpha + (1.0 - f_alpha) * b_alpha;
    if out_alpha <= 1e-6 {
        return background;
    }
    let mix = |shift: u32| -> u32 {
        let f = channel(foreground, shift);
        let b = channel(background, shift);
        (((f * f_alpha + b * b_alpha * (1.0 - f_alpha)) / out_alpha) as u32) & 0xff
    };
    let a = (out_alpha * 255.0) as u32;
    (a << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_digit_is_opaque_and_abgr_by_default() {
        // 官方 weasel.yaml 的 aqua text_color: 0x000000 —— 不透明黑
        assert_eq!(parse("0x000000", ColorFormat::Abgr), Some(0xff00_0000));
        // 官方 azure back_color: 0x8b4e01 → ABGR → RGB(0x01,0x4e,0x8b) 深蓝
        assert_eq!(parse("0x8b4e01", ColorFormat::Abgr), Some(0xff01_4e8b));
    }

    #[test]
    fn eight_digit_keeps_alpha() {
        assert_eq!(parse("0x00000000", ColorFormat::Abgr), Some(0x0000_0000));
        assert_eq!(parse("0x80ff0000", ColorFormat::Abgr), Some(0x8000_00ff));
        assert_eq!(parse("0xfa3a0a", ColorFormat::Abgr), Some(0xff0a_3afa));
    }

    #[test]
    fn short_forms_expand_like_official() {
        assert_eq!(parse("#f00", ColorFormat::Abgr), Some(0xff00_00ff));
        assert_eq!(parse("#8f00", ColorFormat::Abgr), Some(0x8800_00ff));
    }

    #[test]
    fn format_selects_channel_order() {
        assert_eq!(parse("0xff0000", ColorFormat::Abgr), Some(0xff00_00ff));
        assert_eq!(parse("0xff0000", ColorFormat::Argb), Some(0xffff_0000));
        assert_eq!(parse("0xff0000ff", ColorFormat::Rgba), Some(0xffff_0000));
    }

    #[test]
    fn decimal_form_rounds_to_opaque() {
        assert_eq!(parse("16711680", ColorFormat::Abgr), Some(0xff00_00ff));
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(parse("不是颜色", ColorFormat::Abgr), None);
        assert_eq!(parse("0xzz", ColorFormat::Abgr), None);
        assert_eq!(parse("", ColorFormat::Abgr), None);
    }

    #[test]
    fn toml_text_keeps_opaque_and_transparent_apart() {
        assert_eq!(format_text(0xff00_0000, ColorFormat::Abgr), "0x000000");
        assert_eq!(format_text(0x0000_0000, ColorFormat::Abgr), "0x00000000");
        assert_eq!(format_text(0xff0a_3afa, ColorFormat::Abgr), "0xfa3a0a");
        // 往返不失真
        for argb in [0xff00_0000u32, 0x0000_0000, 0xff0a_3afa, 0x80ff_0000] {
            let text = format_text(argb, ColorFormat::Abgr);
            assert_eq!(parse(&text, ColorFormat::Abgr), Some(argb), "往返 {text}");
        }
    }

    #[test]
    fn blend_matches_official_shape() {
        // 不透明前景 → 完全覆盖背景
        assert_eq!(blend(0xff11_2233, 0xffaa_bbcc), 0xff11_2233);
        // 全透明前景 → 背景不变
        assert_eq!(blend(0x0000_0000, 0xffaa_bbcc), 0xffaa_bbcc);
        // 半透明黑压在白底上 → 灰
        let mixed = blend(0x8000_0000, 0xffff_ffff);
        assert_eq!(mixed >> 24, 0xff);
        let red = (mixed >> 16) & 0xff;
        assert!((0x70..=0x90).contains(&red), "半透明黑压白应得中灰，实际 {red:#x}");
    }
}
