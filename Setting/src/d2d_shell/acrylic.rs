//! DWM 未公开合成 API：系统级亚克力（Acrylic）模糊。
//!
//! 与候选窗 `theme/src/accent.rs` 同一套调用：`SetWindowCompositionAttribute` +
//! `WCA_ACCENT_POLICY` 对任意顶层 HWND 生效，DWM 直接在窗口矩形背后做模糊，
//! 上方叠我们 D2D 画的半透明面板 —— 零抓屏、零自写模糊。

#![allow(non_snake_case, non_camel_case_types, dead_code, clippy::all)]

use weasel_theme_support::d2d_bindings::HWND;
use windows_core::BOOL;

/// `WCA_ACCENT_POLICY`：窗口组合属性 id（未公开，全系统稳定为 19）。
const WCA_ACCENT_POLICY: u32 = 19;
/// 经典模糊（Win10 1803+ 通用）。
const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
/// 让上面的模糊走 Acrylic 染色。
const ACCENT_FLAG_ENABLE_ACRYLIC_BLUR_BEHIND: u32 = 0x20;

#[repr(C)]
struct AccentPolicy {
    accent_state: u32,
    accent_flags: u32,
    gradient_color: u32,
    animation_id: u32,
}

#[repr(C)]
struct WinCompAttrData {
    attribute: u32,
    data: *mut core::ffi::c_void,
    size_of_data: u32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowCompositionAttribute(hwnd: HWND, data: *mut WinCompAttrData) -> BOOL;
}

/// `0xAARRGGBB` → DWM 要的 `0xAABBGGRR`。
fn to_accent_color(argb: u32) -> u32 {
    let a = (argb >> 24) & 0xff;
    let r = (argb >> 16) & 0xff;
    let g = (argb >> 8) & 0xff;
    let b = argb & 0xff;
    (a << 24) | (b << 16) | (g << 8) | r
}

/// 给窗口开启系统级亚克力；失败静默忽略（外壳不能因为少一层模糊就起不来）。
pub fn enable_acrylic(hwnd: HWND, tint: u32) {
    unsafe {
        let mut policy = AccentPolicy {
            accent_state: ACCENT_ENABLE_BLURBEHIND,
            accent_flags: ACCENT_FLAG_ENABLE_ACRYLIC_BLUR_BEHIND,
            gradient_color: to_accent_color(tint),
            animation_id: 0,
        };
        let mut data = WinCompAttrData {
            attribute: WCA_ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut core::ffi::c_void,
            size_of_data: core::mem::size_of::<AccentPolicy>() as u32,
        };
        let _ = SetWindowCompositionAttribute(hwnd, &mut data);
    }
}
