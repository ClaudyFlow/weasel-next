//! DWM 未公开合成 API：系统级真毛玻璃（Acrylic 磨砂）。
//!
//! `SetWindowCompositionAttribute` + `WCA_ACCENT_POLICY` 对任意顶层 HWND 都生效，
//! 与窗口是否走 DComp 重定向位图无关（参照 window-vibrancy / TranslucentTB /
//! hufu-ime-rust 的 candwin2.rs）。这一条调用让 DWM 直接在窗口矩形背后做系统级
//! 模糊——零抓屏、零模糊算法。
//!
//! 候选窗本身是 `WS_EX_NOREDIRECTIONBITMAP` + DirectComposition 合成层（见
//! `presenter.rs`）：DWM 把桌面的模糊结果垫在窗口矩形之后，上方再逐像素叠我们
//! 用 D2D 画的（半透明）面板，于是面板本体与四角都透出毛玻璃。

#![allow(non_snake_case, non_camel_case_types, dead_code, clippy::all)]

use windows_core::BOOL;
use crate::d2d_bindings::HWND;

/// `WCA_ACCENT_POLICY`：窗口组合属性 id（未公开，全系统稳定为 19）。
const WCA_ACCENT_POLICY: u32 = 19;
/// 关掉组合（清残留 / 热切换回普通合成）。
const ACCENT_DISABLED: u32 = 0;
/// 经典模糊（Win10 1803+ 全系统通用的毛玻璃），`gradient_color` 用作染色。
const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
/// 让上面的 BLURBEHIND 走 Acrylic 染色（而非纯模糊）。
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

/// 把配色里的 `0xAARRGGBB` 转成 DWM 要的 `0xAABBGGRR`（BGR 字节序，alpha 在高位）。
fn to_accent_color(argb: u32) -> u32 {
    let a = (argb >> 24) & 0xff;
    let r = (argb >> 16) & 0xff;
    let g = (argb >> 8) & 0xff;
    let b = argb & 0xff;
    (a << 24) | (b << 16) | (g << 8) | r
}

/// 给候选窗开启系统级 Acrylic 毛玻璃；`tint` 为 `0xAARRGGBB`，会被转成 DWM 染色。
///
/// 失败静默忽略——毛玻璃是锦上添花，绝不能因为它让输入法崩掉。
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
            size_of_data: std::mem::size_of::<AccentPolicy>() as u32,
        };
        let _ = SetWindowCompositionAttribute(hwnd, &mut data);
    }
}

/// 关掉毛玻璃（热切换回普通合成，清掉残留）。
pub fn disable_acrylic(hwnd: HWND) {
    unsafe {
        let mut policy = AccentPolicy {
            accent_state: ACCENT_DISABLED,
            accent_flags: 0,
            gradient_color: 0,
            animation_id: 0,
        };
        let mut data = WinCompAttrData {
            attribute: WCA_ACCENT_POLICY,
            data: &mut policy as *mut _ as *mut core::ffi::c_void,
            size_of_data: std::mem::size_of::<AccentPolicy>() as u32,
        };
        let _ = SetWindowCompositionAttribute(hwnd, &mut data);
    }
}

/// 根据开关状态统一同步毛玻璃：开则 enable（带 tint），关则 disable。
///
/// 用于主题热切换 / 配置变更时即时更新，不必重建窗口。
pub fn sync_acrylic(hwnd: HWND, glass: bool, tint: u32) {
    if glass {
        enable_acrylic(hwnd, tint);
    } else {
        disable_acrylic(hwnd);
    }
}
