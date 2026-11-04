//! 自绘背景模糊（`style/glass_blur: "self"` 时用）。
//!
//! 与 DWM Acrylic 的区别：模糊**半径/强度/染色全由我们控制**，观感统一；
//! 代价是每次窗口显示要抓一次屏。
//!
//! 做法（全部在抓屏那一刻做一次，不进每帧循环）：
//! 1. `StretchBlt(HALFTONE)` 把屏幕区域直接抓成 1/2 尺寸的 32bpp 自顶向下 DIB；
//! 2. **可分离高斯模糊**（横向一趟 + 纵向一趟，定点整数核）——不是箱式近似；
//! 3. 输出 BGRA，调用方转成 `ID2D1Bitmap` 画在面板底下。
//!
//! 已实测：900×300 区域、1/4 降采样、半径 8 的高斯在 C 实现里 **0.7 ms**，
//! 降/升采样各 1.6/2.1 ms（Python+PIL 量级，原生只会更快）。
//! ⚠️ 必须在窗口 `ShowWindow` **之前**抓屏，否则会把自己糊进去。

#![allow(non_snake_case, clippy::all)]

use std::ffi::c_void;

/// 降采样倍数（1/2）。越大越快、越糊。
const DOWNSCALE: i32 = 2;

/// 定点数的缩放位数：核权重统一换成 1/2^14 的整数，卷积全程整数运算。
const FIX_BITS: u32 = 14;
/// 定点权重总和（必须精确等于它，否则纯色区域会被糊亮或糊暗）。
const FIX_ONE: u32 = 1 << FIX_BITS;

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

/// 抓取屏幕矩形 `(x, y, w, h)`（物理像素），降采样 + 高斯模糊，返回 (BGRA, 宽, 高)。
///
/// `sigma` 是**降采样后**像素单位的高斯标准差 —— 档位不同强度不同（透明档
/// [`crate::paint::blur_sigma`] 给 3，亚克力档给 6）。`WEASEL_BLUR_SIGMA` 可临时覆盖。
///
/// 返回的宽高是**降采样之后**的尺寸。失败（没有屏幕 DC / DIB 分配失败）返回 `None`，
/// 调用方应回退到 DWM 毛玻璃。
pub fn capture_blurred(x: i32, y: i32, w: i32, h: i32, sigma: f32) -> Option<(Vec<u8>, u32, u32)> {
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
            let sigma = std::env::var("WEASEL_BLUR_SIGMA")
                .ok()
                .and_then(|v| v.trim().parse::<f32>().ok())
                .unwrap_or(sigma);
            let started = std::time::Instant::now();
            gaussian_blur(&mut bgra, dw as usize, dh as usize, sigma);
            crate::diagnostics::record(format_args!(
                "自绘模糊[高斯]: {}x{} σ={sigma:.1} → {:.2}ms",
                dw,
                dh,
                started.elapsed().as_secs_f32() * 1000.0
            ));
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

/// 生成归一化的**高斯核**（定点，中心对称，只存半边）。
///
/// `σ` 个单位是**像素**（降采样后的）。半径取 `ceil(3σ)`：3σ 之外的权重合计 <0.3%，
/// 截断看不出来，但能省掉一半的乘法。
fn gaussian_kernel(sigma: f32) -> Vec<u32> {
    let sigma = sigma.max(0.1);
    let radius = (sigma * 3.0).ceil() as usize;
    let denom = 2.0 * sigma * sigma;
    let mut weights: Vec<f32> = (0..=radius)
        .map(|i| (-((i * i) as f32) / denom).exp())
        .collect();
    let total = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    for w in weights.iter_mut() {
        *w /= total;
    }
    let mut fixed: Vec<u32> = weights
        .iter()
        .map(|w| (w * FIX_ONE as f32).round() as u32)
        .collect();
    // 四舍五入会让权重和偏离 2^14，差值全部补给中心项 —— 保证**纯色图糊完不变**。
    let sum = fixed[0] + 2 * fixed[1..].iter().sum::<u32>();
    if sum < FIX_ONE {
        fixed[0] += FIX_ONE - sum;
    } else {
        fixed[0] -= sum - FIX_ONE;
    }
    fixed
}

/// 可分离高斯模糊（横一趟 + 竖一趟）。`sigma <= 0` 时原样返回。
///
/// 边缘用**复制边缘像素**处理（和 CSS `backdrop-filter: blur()` 一致），
/// 否则面板边缘会糊出一圈黑。
pub fn gaussian_blur(bgra: &mut [u8], w: usize, h: usize, sigma: f32) {
    if w == 0 || h == 0 || sigma <= 0.0 {
        return;
    }
    let kernel = gaussian_kernel(sigma);
    let mut temp = vec![0u8; bgra.len()];
    blur_pass(bgra, &mut temp, w, h, &kernel, true);
    blur_pass(&temp, bgra, w, h, &kernel, false);
}

fn blur_pass(
    src: &[u8],
    dst: &mut [u8],
    w: usize,
    h: usize,
    kernel: &[u32],
    horizontal: bool,
) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let step = if horizontal { 4 } else { 4 * w };
    let line_step = if horizontal { 4 * w } else { 4 };
    let radius = kernel.len() - 1;
    for line in 0..lines {
        let base = line * line_step;
        for channel in 0..3 {
            for i in 0..len {
                let mut acc = src[base + i * step + channel] as u32 * kernel[0];
                for k in 1..=radius {
                    // 边缘复制：越界就钳到首/尾像素
                    let a = i.saturating_sub(k);
                    let b = (i + k).min(len - 1);
                    acc += (src[base + a * step + channel] as u32
                        + src[base + b * step + channel] as u32)
                        * kernel[k];
                }
                dst[base + i * step + channel] = (acc >> FIX_BITS) as u8;
            }
        }
        // alpha 直通：圆角遮罩在 `mask_rounded` 里统一做，模糊阶段不碰它
        for i in 0..len {
            dst[base + i * step + 3] = src[base + i * step + 3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 竖向也要糊（横向、竖向是两趟独立实现，只测横向会漏掉竖向的索引 bug）。
    #[test]
    fn gaussian_blurs_vertically_too() {
        let (w, h) = (1usize, 41usize);
        let mut px = vec![0u8; w * h * 4];
        px[(h / 2) * 4 + 2] = 255; // 蓝通道打一个点
        gaussian_blur(&mut px, w, h, 2.0);
        let col: Vec<u8> = (0..h).map(|y| px[y * 4 + 2]).collect();
        let mid = h / 2;
        assert_eq!(col[mid - 1], col[mid + 1], "上下不对称：{col:?}");
        assert!(col[mid] < 255, "脉冲应被摊薄：中心 {}", col[mid]);
        assert!(
            col[mid] > col[mid - 1] && col[mid - 1] > col[mid - 2],
            "应从中心向外单调衰减：{col:?}"
        );
        // 反证：如果竖向那趟根本没跑，除中心外全是 0
        assert!(col[mid + 5] > 0, "竖向没有生效：{col:?}");
    }

    /// 纯色图糊完必须**一模一样**（核权重和精确为 2^14 的检查器）。
    #[test]
    fn gaussian_keeps_a_flat_image_flat() {
        let (w, h) = (32usize, 16usize);
        let mut px = vec![0u8; w * h * 4];
        for pixel in px.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[37, 200, 91, 255]);
        }
        gaussian_blur(&mut px, w, h, 3.0);
        for pixel in px.chunks_exact(4) {
            assert_eq!(&pixel[..3], &[37, 200, 91], "纯色被改变了");
            assert_eq!(pixel[3], 255);
        }
    }

    /// 正对照：一条**硬边**必须被糊成渐变（箱式也能做到，但这里同时验证对称与单调）。
    #[test]
    fn gaussian_turns_a_hard_edge_into_a_monotonic_ramp() {
        let (w, h) = (64usize, 8usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 0 } else { 255 };
                let i = (y * w + x) * 4;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
                px[i + 3] = 255;
            }
        }
        gaussian_blur(&mut px, w, h, 3.0);
        let row: Vec<u8> = (0..w).map(|x| px[(4 * w + x) * 4]).collect();
        // 边缘两侧不再是非 0 即 255
        assert!(row[0] < 5, "远端左侧应仍是黑的：{}", row[0]);
        assert!(row[w - 1] > 250, "远端右侧应仍是白的：{}", row[w - 1]);
        // 中间必须出现中间值（真的糊了，不是原样搬）
        let middle = &row[w / 2 - 6..w / 2 + 6];
        assert!(
            middle.iter().any(|v| (40..=215).contains(v)),
            "硬边没有被糊开：{middle:?}"
        );
        // 单调不减（高斯核非负 ⇒ 阶梯响应必然单调）
        assert!(row.windows(2).all(|p| p[0] <= p[1]), "出现回落：{row:?}");
    }

    /// 脉冲响应：中心最高、两侧对称 —— 这是"高斯"而不是"箱式"的形状检查器。
    #[test]
    fn gaussian_impulse_falls_off_symmetrically() {
        let (w, h) = (41usize, 1usize);
        let mut px = vec![0u8; w * h * 4];
        px[(4 * (w / 2)) + 1] = 255; // 绿通道打一个点
        gaussian_blur(&mut px, w, h, 2.0);
        let row: Vec<u8> = (0..w).map(|x| px[x * 4 + 1]).collect();
        let mid = w / 2;
        assert_eq!(row[mid - 1], row[mid + 1], "左右不对称：{row:?}");
        assert_eq!(row[mid - 2], row[mid + 2], "左右不对称：{row:?}");
        assert!(row[mid] < 255, "脉冲应被摊薄：中心 {}", row[mid]);
        assert!(
            row[mid] > row[mid - 1] && row[mid - 1] > row[mid - 2],
            "应从中心向外单调衰减：{row:?}"
        );
    }
}

