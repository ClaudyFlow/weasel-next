//! 抓屏 + 三趟箱式模糊（≈高斯），给候选窗当亚克力背景。
//!
//! 从 `theme/src/backdrop.rs` 抄过来的最小版：那边是 cdylib，不能当库依赖。

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, SetStretchBltMode, StretchBlt, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CAPTUREBLT, DIB_RGB_COLORS, HALFTONE, SRCCOPY,
};

const DOWNSCALE: i32 = 2;
const BLUR_RADIUS: usize = 6;

/// 抓 `(x, y, w, h)` 那块屏幕，降采样后模糊，返回 BGRA（自顶向下）。
pub unsafe fn capture_blurred(x: i32, y: i32, w: i32, h: i32) -> Option<(Vec<u8>, u32, u32)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let dw = (w / DOWNSCALE).max(1);
    let dh = (h / DOWNSCALE).max(1);
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: dw,
                biHeight: -dh,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let Ok(bitmap) = windows::Win32::Graphics::Gdi::CreateDIBSection(
            Some(dc),
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        ) else {
            let _ = DeleteDC(dc);
            let _ = ReleaseDC(None, screen);
            return None;
        };
        let old = SelectObject(dc, bitmap.into());
        let _ = SetStretchBltMode(dc, HALFTONE);
        let _ = StretchBlt(dc, 0, 0, dw, dh, Some(screen), x, y, w, h, SRCCOPY | CAPTUREBLT);
        let _ = BitBlt(dc, 0, 0, dw, dh, Some(screen), x, y, SRCCOPY | CAPTUREBLT);

        let mut pixels = vec![0u8; (dw * dh * 4) as usize];
        std::ptr::copy_nonoverlapping(bits as *const u8, pixels.as_mut_ptr(), pixels.len());
        let _ = SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        let _ = ReleaseDC(None, screen);

        box_blur(&mut pixels, dw as usize, dh as usize, BLUR_RADIUS);
        Some((pixels, dw as u32, dh as u32))
    }
}

/// 三趟可分离箱式模糊（≈高斯）。
pub fn box_blur(pixels: &mut [u8], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let mut scratch = vec![0u8; pixels.len()];
    for _ in 0..3 {
        // 水平
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 4];
                let mut count = 0u32;
                for k in -(radius as i32)..=(radius as i32) {
                    let sx = (x as i32 + k).clamp(0, w as i32 - 1) as usize;
                    let index = (y * w + sx) * 4;
                    for channel in 0..4 {
                        acc[channel] += pixels[index + channel] as u32;
                    }
                    count += 1;
                }
                let index = (y * w + x) * 4;
                for channel in 0..4 {
                    scratch[index + channel] = (acc[channel] / count) as u8;
                }
            }
        }
        // 垂直
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0u32; 4];
                let mut count = 0u32;
                for k in -(radius as i32)..=(radius as i32) {
                    let sy = (y as i32 + k).clamp(0, h as i32 - 1) as usize;
                    let index = (sy * w + x) * 4;
                    for channel in 0..4 {
                        acc[channel] += scratch[index + channel] as u32;
                    }
                    count += 1;
                }
                let index = (y * w + x) * 4;
                for channel in 0..4 {
                    pixels[index + channel] = (acc[channel] / count) as u8;
                }
            }
        }
    }
}
