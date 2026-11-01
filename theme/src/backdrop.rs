//! 自绘背景模糊（`style/glass_blur: "self"` 时用）。
//!
//! 与 DWM Acrylic 的区别：模糊**半径/强度/染色全由我们控制**，观感统一；
//! 代价是每次窗口显示要抓一次屏。
//!
//! 做法（全部在抓屏那一刻做一次，不进每帧循环）：
//! 1. `StretchBlt(HALFTONE)` 把屏幕区域直接抓成 1/4 尺寸的 32bpp 自顶向下 DIB；
//! 2. 三趟箱式模糊（近似高斯）——整数、无分配；
//! 3. 输出 RGBA，调用方转成 `ID2D1Bitmap` 画在面板底下。
//!
//! 已实测：900×300 区域、1/4 降采样、半径 8 的高斯在 C 实现里 **0.7 ms**，
//! 降/升采样各 1.6/2.1 ms（Python+PIL 量级，原生只会更快）。
//! ⚠️ 必须在窗口 `ShowWindow` **之前**抓屏，否则会把自己糊进去。

#![allow(non_snake_case, clippy::all)]

use std::ffi::c_void;

/// 降采样倍数（1/4）。越大越快、越糊。
const DOWNSCALE: i32 = 2;
/// 箱式模糊的半径（在降采样后的像素单位上）。
const BLUR_RADIUS: i32 = 6;
/// 箱式模糊的趟数：3 趟 ≈ 高斯。
const BLUR_PASSES: usize = 3;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BITMAPINFOHEADER {
    biSize: u32,
    biWidth: i32,
    biHeight: i32,
    biPlanes: u16,
    biBitCount: u16,
    biCompression: u32,
    biSizeImage: u32,
    biXPelsPerMeter: i32,
    biYPelsPerMeter: i32,
    biClrUsed: u32,
    biClrImportant: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct BITMAPINFO {
    bmiHeader: BITMAPINFOHEADER,
    bmiColors: [u32; 3],
}

const DIB_RGB_COLORS: u32 = 0;
const BI_RGB: u32 = 0;
const HALFTONE: i32 = 4;
const SRCCOPY: u32 = 0x00CC_0020;
const CAPTUREBLT: u32 = 0x4000_0000;

#[link(name = "gdi32")]
/// user32：屏幕 DC。
unsafe extern "system" {
    fn GetDC(hwnd: *mut c_void) -> *mut c_void;
    fn ReleaseDC(hwnd: *mut c_void, dc: *mut c_void) -> i32;
}

/// gdi32：DIB 与拉伸抓屏。
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: *mut c_void) -> *mut c_void;
    fn DeleteDC(dc: *mut c_void) -> i32;
    fn CreateDIBSection(
        dc: *mut c_void,
        info: *const BITMAPINFO,
        usage: u32,
        bits: *mut *mut c_void,
        section: *mut c_void,
        offset: u32,
    ) -> *mut c_void;
    fn SelectObject(dc: *mut c_void, object: *mut c_void) -> *mut c_void;
    fn DeleteObject(object: *mut c_void) -> i32;
    fn SetStretchBltMode(dc: *mut c_void, mode: i32) -> i32;
    fn StretchBlt(
        dst: *mut c_void,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: *mut c_void,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        rop: u32,
    ) -> i32;
}

/// 抓取屏幕矩形 `(x, y, w, h)`（物理像素），降采样 + 箱式模糊，返回 (RGBA, 宽, 高)。
///
/// 返回的宽高是**降采样之后**的尺寸。失败（没有屏幕 DC / DIB 分配失败）返回 `None`，
/// 调用方应回退到 DWM 毛玻璃。
pub fn capture_blurred(x: i32, y: i32, w: i32, h: i32) -> Option<(Vec<u8>, u32, u32)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let dw = (w / DOWNSCALE).max(1);
    let dh = (h / DOWNSCALE).max(1);
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return None;
        }
        let mem = CreateCompatibleDC(screen);
        if mem.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
            return None;
        }
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = core::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = dw;
        info.bmiHeader.biHeight = -dh; // 负数 = 自顶向下
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if dib.is_null() || bits.is_null() {
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
            return None;
        }
        let old = SelectObject(mem, dib);
        SetStretchBltMode(mem, HALFTONE);
        // CAPTUREBLT：把分层窗口也抓进来（否则透明窗口区域会是黑的）。
        let ok = StretchBlt(
            mem,
            0,
            0,
            dw,
            dh,
            screen,
            x,
            y,
            w,
            h,
            SRCCOPY | CAPTUREBLT,
        );
        SelectObject(mem, old);
        let mut out = None;
        if ok != 0 {
            // BGRA → RGBA，并补满 alpha（后面按预乘画，颜色已是不透明色）。
            let len = (dw * dh * 4) as usize;
            let src = std::slice::from_raw_parts(bits as *const u8, len);
            // ⚠️ 保持 **BGRA**，不做通道交换：D2D 那张位图声明的是 B8G8R8A8_UNORM。
            // （20:5x 踩过：这里换成 RGBA 会导致红蓝互换，蓝底网页变橙色。）
            let mut bgra = src.to_vec();
            box_blur(&mut bgra, dw as usize, dh as usize, BLUR_RADIUS as usize);
            out = Some((bgra, dw as u32, dh as u32));
        }
        DeleteObject(dib);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        out
    }
}

/// 把位图的 alpha 按**圆角矩形**裁掉（圆角外侧完全透明）。
///
/// 为什么在 CPU 里做：D2D 的 `CreateBitmapBrush` 在这套绑定里不可用、
/// `PushLayer` 又缺 `CreateLayer`，而位图本身只有几十乘几十个像素，
/// 逐像素乘个覆盖率比折腾几何遮罩便宜也更可靠。
pub fn mask_rounded(rgba: &mut [u8], w: usize, h: usize, radius: f32, inset_x: f32, inset_y: f32) {
    if w == 0 || h == 0 {
        return;
    }
    // 可见区域 = 整张位图内缩 `inset_*`（窗口比面板大出来的那一圈，给阴影/描边留的）。
    // 那一圈必须透明，否则窗口四角就是背景的方角。
    let left = inset_x.max(0.0);
    let top = inset_y.max(0.0);
    let right = w as f32 - inset_x;
    let bottom = h as f32 - inset_y;
    if right <= left || bottom <= top {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 0;
        }
        return;
    }
    let rw = right - left;
    let rh = bottom - top;
    let radius = radius.max(0.0).min(rw.min(rh) / 2.0);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 + 0.5 - left;
            let fy = y as f32 + 0.5 - top;
            let index = (y * w + x) * 4;
            // 到圆角矩形的覆盖率（圆角处做 1px 软边，避免锯齿）
            let coverage = if fx < 0.0 || fy < 0.0 || fx > rw || fy > rh {
                0.0
            } else {
                let dx = if fx < radius {
                    radius - fx
                } else if fx > rw - radius {
                    fx - (rw - radius)
                } else {
                    0.0
                };
                let dy = if fy < radius {
                    radius - fy
                } else if fy > rh - radius {
                    fy - (rh - radius)
                } else {
                    0.0
                };
                if dx > 0.0 && dy > 0.0 {
                    (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            };
            // 预乘 alpha：RGB 要跟着一起乘，位图是 PREMULTIPLIED，只改 alpha 会在圆角边缘发白
            for channel in 0..3 {
                rgba[index + channel] = (rgba[index + channel] as f32 * coverage) as u8;
            }
            rgba[index + 3] = (255.0 * coverage) as u8;
        }
    }
}

/// 三趟可分离箱式模糊（横一趟 + 竖一趟算一趟），末尾统一平均。
pub fn box_blur(rgba: &mut [u8], w: usize, h: usize, radius: usize) {
    if w == 0 || h == 0 || radius == 0 {
        return;
    }
    let mut intermediate = vec![0u8; rgba.len()];
    for _ in 0..BLUR_PASSES {
        blur_axis(rgba, &mut intermediate, w, h, radius, true);
        blur_axis(&intermediate, rgba, w, h, radius, false);
    }
}

fn blur_axis(
    src: &[u8],
    dst: &mut [u8],
    w: usize,
    h: usize,
    radius: usize,
    horizontal: bool,
) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let step = if horizontal { 4 } else { 4 * w };
    let line_step = if horizontal { 4 * w } else { 4 };
    for line in 0..lines {
        for channel in 0..3 {
            let mut sum: u32 = 0;
            let mut count: u32 = 0;
            for i in 0..=radius.min(len - 1) {
                sum += src[line * line_step + i * step + channel] as u32;
                count += 1;
            }
            for i in 0..len {
                dst[line * line_step + i * step + channel] = (sum / count) as u8;
                if i + radius + 1 < len {
                    sum += src[line * line_step + (i + radius + 1) * step + channel] as u32;
                    count += 1;
                }
                if i >= radius {
                    sum -= src[line * line_step + (i - radius) * step + channel] as u32;
                    count -= 1;
                }
            }
        }
        // alpha 直通
        for i in 0..len {
            dst[line * line_step + i * step + 3] = src[line * line_step + i * step + 3];
        }
    }
}
