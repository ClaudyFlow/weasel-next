//! 用 GDI 把整个设置界面（矩形 + 文字）画进一张 32bpp DIB。
//!
//! 为什么先用 GDI 而不是 DirectWrite 字形图集：这一步要验的是「Vulkan 能把
//! 一张带文字的画布贴到窗口上」，GDI 的中文渲染（灰度抗锯齿）已经够看，
//! 而且几十行就能出图。等布局定稿再换 DirectWrite 图集 + 真 shader。
//!
//! 当前是「固定不透明灰色」主题：整套配色走灰阶（R=G=B），不做玻璃、不做亚克力、
//! 不接主题链。先把「窗口 → 画布 → 交换链 → 呈现」这条链路在稳定底色上跑通。
//!
//! 内容照搬自 `Setting/ui/*.slint`（小狼毫Next 设置）：7 个页签（主页 / 界面风格 /
//! 方案选单 / 常规 / 更新 / 词库 / 下载）。

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteObject, FillRect,
    GetTextExtentPoint32W, RoundRect, SelectObject, SetBkMode, SetTextColor, TextOutW, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBRUSH, HDC, HFONT, TRANSPARENT,
};

// ── 固定不透明灰色配色（GDI COLORREF = 0x00BBGGRR）──────────────────────────────
const C_BG: u32 = 0x002B2B2B; // 窗口底色：中深灰，完全不透明
const C_TOPBAR: u32 = 0x00333333; // 顶部标题条
const C_NAV: u32 = 0x003A3A3A; // 左侧导航：未选中
const C_NAV_SEL: u32 = 0x004E4E4E; // 左侧导航：选中项
const C_CARD: u32 = 0x003C3C3C; // 卡片底色
const C_CARD2: u32 = 0x00454545; // 输入框 / 列表底色（更亮一档）
const C_TEXT: u32 = 0x00F0F0F0; // 主文字（亮）
const C_TEXT_DIM: u32 = 0x00B8B8B8; // 次要文字
const C_TEXT_SUB: u32 = 0x00A0A0A0; // 注释 / 灰历史值
const C_TRACK: u32 = 0x00505050; // 进度条轨道
const C_FILL: u32 = 0x009A9A9A; // 进度条已填充
const C_LINE: u32 = 0x00454545; // 边框线

/// 画好的画布：BGRA（自顶向下），宽高按像素。
pub struct Canvas {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

impl Canvas {
    /// 按 1250×825 的逻辑坐标、给定 DPI 缩放、给定页码画一版。
    ///
    /// `scale` 由调用方按**交换链真实宽度**算出（`extent.width / 1250.0`），
    /// 这样画布像素尺寸和交换链严丝合缝——写死 1.25 会在 100% DPI 的机器上
    /// 画出 1562×1031、比窗口大一圈，呈现时只能贴左上角一块（00:4x 实测）。
    ///
    /// `page` 对应 7 个页签：0 主页 / 1 界面风格 / 2 方案选单 / 3 常规 / 4 更新 / 5 词库 / 6 下载。
    pub unsafe fn draw_settings(scale: f32, page: usize) -> Self {
        let width = (1250.0 * scale) as i32;
        let height = (825.0 * scale) as i32;

        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // 负数 = 自顶向下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let old_bitmap = SelectObject(dc, bitmap.into());
        let _ = SetBkMode(dc, TRANSPARENT);

        let px = |dip: f32| (dip * scale) as i32;

        // ── 背景 ──
        fill(dc, 0.0, 0.0, 1250.0, 825.0, C_BG, scale);

        // ── 顶部标题条 ──
        fill(dc, 0.0, 0.0, 1250.0, 60.0, C_TOPBAR, scale);
        draw_text(dc, "小狼毫Next 设置", 24.0, 16.0, 24.0, C_TEXT, true, scale);

        // ── 左侧导航（7 页，与 Slint 标签一致）──
        let nav = [
            "主页",
            "界面风格",
            "方案选单",
            "常规",
            "更新",
            "词库",
            "下载",
        ];
        for (index, label) in nav.iter().enumerate() {
            let y = 76.0 + index as f32 * 50.0;
            let sel = index == page;
            fill(dc, 16.0, y, 132.0, 42.0, if sel { C_NAV_SEL } else { C_NAV }, scale);
            // 选中项左侧一条亮条
            if sel {
                fill(dc, 16.0, y, 4.0, 42.0, 0x0087CEEB, scale);
            }
            draw_text_center(
                dc,
                label,
                16.0,
                y,
                132.0,
                42.0,
                16.0,
                if sel { C_TEXT } else { C_TEXT_DIM },
                true,
                scale,
            );
        }

        // ── 内容区分发 ──
        let cx = 164.0;
        let cy = 70.0;
        let cw = 1250.0 - cx - 20.0; // 内容宽
        match page {
            0 => draw_home(dc, cx, cy, cw, scale),
            1 => draw_schemes(dc, cx, cy, cw, scale),
            2 => draw_schemas(dc, cx, cy, cw, scale),
            3 => draw_general(dc, cx, cy, cw, scale),
            4 => draw_update(dc, cx, cy, cw, scale),
            5 => draw_dict(dc, cx, cy, cw, scale),
            6 => draw_download(dc, cx, cy, cw, scale),
            _ => {}
        }

        // 取像素
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        std::ptr::copy_nonoverlapping(bits as *const u8, pixels.as_mut_ptr(), pixels.len());

        let _ = SelectObject(dc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);

        Self {
            width,
            height,
            pixels,
        }
    }

    /// 把当前画布写成 24bpp BMP 文件（无窗口环境下预览界面用）。
    ///
    /// 画布像素是 BGRA 自顶向下；BMP 要求自底向上，所以写文件时按行反转。
    pub fn save_bmp(&self, path: &str) -> std::io::Result<()> {
        use std::fs::File;
        use std::io::Write;
        let w = self.width as usize;
        let h = self.height as usize;
        let row_bytes = w * 3;
        let padding = (4 - (row_bytes % 4)) % 4;
        let stride = row_bytes + padding;
        let mut pixel_data = Vec::with_capacity(stride * h);
        for y in (0..h).rev() {
            let row_start = y * w * 4;
            for x in 0..w {
                let o = row_start + x * 4;
                pixel_data.push(self.pixels[o]); // B
                pixel_data.push(self.pixels[o + 1]); // G
                pixel_data.push(self.pixels[o + 2]); // R
            }
            for _ in 0..padding {
                pixel_data.push(0);
            }
        }
        let data_size = pixel_data.len() as u32;
        let file_size = 14 + 40 + data_size;
        let mut header = Vec::with_capacity(file_size as usize);
        header.extend_from_slice(b"BM");
        header.extend_from_slice(&file_size.to_le_bytes());
        header.extend_from_slice(&[0u8; 4]); // reserved
        header.extend_from_slice(&(14u32 + 40).to_le_bytes()); // 像素数据偏移
        header.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER 大小
        header.extend_from_slice(&(w as i32).to_le_bytes());
        header.extend_from_slice(&(h as i32).to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes()); // planes
        header.extend_from_slice(&24u16.to_le_bytes()); // bpp
        header.extend_from_slice(&0u32.to_le_bytes()); // compression
        header.extend_from_slice(&data_size.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes()); // x ppm
        header.extend_from_slice(&0u32.to_le_bytes()); // y ppm
        header.extend_from_slice(&0u32.to_le_bytes()); // colors used
        header.extend_from_slice(&0u32.to_le_bytes()); // important colors
        let mut file = File::create(path)?;
        file.write_all(&header)?;
        file.write_all(&pixel_data)?;
        Ok(())
    }
}

// ── 绘制工具 ────────────────────────────────────────────────────────────────

/// 填矩形（逻辑坐标 → 像素）。
unsafe fn fill(dc: HDC, x: f32, y: f32, w: f32, h: f32, color: u32, scale: f32) {
    let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(color));
    let rect = RECT {
        left: (x * scale) as i32,
        top: (y * scale) as i32,
        right: ((x + w) * scale) as i32,
        bottom: ((y + h) * scale) as i32,
    };
    FillRect(dc, &rect, brush);
    let _ = DeleteObject(brush.into());
}

/// 填圆角矩形（card / 输入框 / 按钮 用）。
unsafe fn fill_r(dc: HDC, x: f32, y: f32, w: f32, h: f32, color: u32, radius: f32, scale: f32) {
    let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(color));
    let rect = RECT {
        left: (x * scale) as i32,
        top: (y * scale) as i32,
        right: ((x + w) * scale) as i32,
        bottom: ((y + h) * scale) as i32,
    };
    let r = (radius * scale) as i32;
    RoundRect(dc, rect.left, rect.top, rect.right, rect.bottom, r, r);
    let _ = DeleteObject(brush.into());
}

/// 左对齐文字（逻辑坐标 top-left 起画）。
unsafe fn draw_text(dc: HDC, content: &str, x: f32, y: f32, size: f32, color: u32, bold: bool, scale: f32) {
    let font = make_font(size, color, bold, scale);
    let old = SelectObject(dc, font.into());
    let wide: Vec<u16> = content.encode_utf16().collect();
    let _ = TextOutW(dc, (x * scale) as i32, (y * scale) as i32, &wide);
    let _ = SelectObject(dc, old);
    let _ = DeleteObject(font.into());
}

/// 居中文字（在矩形框内水平 + 垂直居中）。
unsafe fn draw_text_center(
    dc: HDC,
    content: &str,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    size: f32,
    color: u32,
    bold: bool,
    scale: f32,
) {
    let font = make_font(size, color, bold, scale);
    let old = SelectObject(dc, font.into());
    let wide: Vec<u16> = content.encode_utf16().collect();
    let mut extent = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &wide, &mut extent);
    let off_x = ((w * scale - extent.cx as f32) / 2.0).max(0.0);
    let off_y = ((h * scale - extent.cy as f32) / 2.0).max(0.0);
    let _ = TextOutW(
        dc,
        (x * scale) as i32 + off_x as i32,
        (y * scale) as i32 + off_y as i32,
        &wide,
    );
    let _ = SelectObject(dc, old);
    let _ = DeleteObject(font.into());
}

/// 量出文字尺寸（用于卡片内布局 / 截断判断）。
unsafe fn text_size(dc: HDC, content: &str, size: f32, bold: bool, scale: f32) -> (i32, i32) {
    let font = make_font(size, 0, bold, scale);
    let old = SelectObject(dc, font.into());
    let wide: Vec<u16> = content.encode_utf16().collect();
    let mut extent = SIZE::default();
    let _ = GetTextExtentPoint32W(dc, &wide, &mut extent);
    let _ = SelectObject(dc, old);
    let _ = DeleteObject(font.into());
    (extent.cx, extent.cy)
}

/// 建一个 GDI 字体（灰度抗锯齿已在 DC 上通过 FONT_QUALITY(4) 生效）。
unsafe fn make_font(size: f32, color: u32, bold: bool, scale: f32) -> HFONT {
    let _ = color; // 颜色由调用方 SetTextColor 设置
    CreateFontW(
        (size * scale) as i32,
        0,
        0,
        0,
        if bold { 700 } else { 400 },
        0,
        0,
        0,
        windows::Win32::Graphics::Gdi::FONT_CHARSET(1), // DEFAULT_CHARSET
        windows::Win32::Graphics::Gdi::FONT_OUTPUT_PRECISION(0),
        windows::Win32::Graphics::Gdi::FONT_CLIP_PRECISION(0),
        windows::Win32::Graphics::Gdi::FONT_QUALITY(4), // ANTIALIASED_QUALITY：灰度抗锯齿
        0,
        w!("Microsoft YaHei UI"),
    )
}

/// 自绘按钮（圆角矩形 + 居中文字）。
unsafe fn button(
    dc: HDC,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    bg: u32,
    text: u32,
    scale: f32,
) {
    fill_r(dc, x, y, w, h, bg, 6.0, scale);
    draw_text_center(dc, label, x, y, w, h, 14.0, text, false, scale);
}

/// 开关（toggle）：开 = 亮色胶囊 + 右滑块；关 = 暗胶囊 + 左滑块。
unsafe fn toggle(dc: HDC, x: f32, y: f32, w: f32, h: f32, on: bool, scale: f32) {
    fill_r(dc, x, y, w, h, if on { 0x0064B5F6 } else { C_CARD2 }, h / 2.0, scale);
    let knob = if on { x + w - h } else { x };
    fill_r(dc, knob + 3.0, y + 3.0, h - 6.0, h - 6.0, 0x00F0F0F0, (h - 6.0) / 2.0, scale);
}

/// 复选框 / 勾选：选中填亮色 + 勾；未选填白底描边。
unsafe fn checkbox(dc: HDC, x: f32, y: f32, s: f32, checked: bool, scale: f32) {
    fill_r(
        dc,
        x,
        y,
        s,
        s,
        if checked { 0x0064B5F6 } else { 0x00F0F0F0 },
        3.0,
        scale,
    );
    if checked {
        // 简单画一个「✓」用两条线太麻烦，这里用文字勾替代
        draw_text(
            dc,
            "✓",
            x + s / 2.0 - 5.0,
            y + s / 2.0 - 9.0,
            16.0,
            0x00FFFFFF,
            true,
            scale,
        );
    }
}

/// 输入框（圆角底 + 占位文字）。
unsafe fn input(dc: HDC, x: f32, y: f32, w: f32, h: f32, placeholder: &str, scale: f32) {
    fill_r(dc, x, y, w, h, C_CARD2, 6.0, scale);
    draw_text(dc, placeholder, x + 10.0, y + h / 2.0 - 9.0, 14.0, C_TEXT_SUB, false, scale);
}

/// 段位进度条（轨道 + 已填充 + 文本）。
unsafe fn progress(dc: HDC, x: f32, y: f32, w: f32, h: f32, ratio: f32, scale: f32) {
    fill_r(dc, x, y, w, h, C_TRACK, h / 2.0, scale);
    fill_r(dc, x, y, w * ratio, h, 0x0064B5F6, h / 2.0, scale);
}

// ── 各页内容 ──────────────────────────────────────────────────────────────

/// 主页（个人生涯 / 统计）—— 照搬自 stats.slint：12 张数字卡 + 段位 + 年度热力图。
unsafe fn draw_home(dc: HDC, cx: f32, cy: f32, cw: f32, scale: f32) {
    draw_text(dc, "个人生涯", cx, cy, 28.0, C_TEXT, true, scale);
    draw_text(
        dc,
        "本机记录，不上传",
        cx,
        cy + 34.0,
        16.0,
        C_TEXT_SUB,
        false,
        scale,
    );

    // 12 张数字卡（3 列 × 4 行）
    let cards: [( &str, &str, &str, u32); 12] = [
        ("上屏字数", "3,303", "累计 9,160", 0x004ADE80),
        ("平均每日字数", "763", "近7天 1,059", 0x00A3E635),
        ("击键", "21,175", "累计 56,768", 0x00F0F0F0),
        ("平均速度", "58", "近7天 61", 0x004ADE80),
        ("最大速度", "73", "字/分", 0x00F87171),
        ("打字时间", "56", "累计 169", 0x002DD4BF),
        ("平均击键", "6.2", "最快 7.1", 0x0038BDF8),
        ("最大击键", "8.4", "键/秒", 0x00FBBF24),
        ("键准", "77.4%", "近7天 81.8%", 0x0038BDF8),
        ("平均码长", "6.41", "近7天 5.98", 0x00A78BFA),
        ("平均词长", "2.35", "字/轮", 0x00FB923C),
        ("连续使用", "1", "最长 11", 0x004ADE80),
    ];
    let gap = 12.0f32;
    let card_w = (cw - 2.0 * gap) / 3.0;
    let card_h = 78.0f32;
    let top = cy + 60.0;
    for (i, (title, value, note, accent)) in cards.iter().enumerate() {
        let col = (i % 3) as f32;
        let row = (i / 3) as f32;
        let x = cx + col * (card_w + gap);
        let y = top + row * (card_h + gap);
        fill_r(dc, x, y, card_w, card_h, C_CARD, 12.0, scale);
        draw_text(dc, title, x + 14.0, y + 10.0, 16.0, C_TEXT_DIM, false, scale);
        draw_text(dc, value, x + 14.0, y + 30.0, 30.0, *accent, true, scale);
        draw_text(dc, note, x + 14.0, y + 56.0, 13.0, C_TEXT_SUB, false, scale);
    }

    // 段位卡
    let cy2 = top + 4.0 * (card_h + gap) + 8.0;
    let card_h2 = 96.0f32;
    fill_r(dc, cx, cy2, cw, card_h2, C_CARD, 12.0, scale);
    draw_text(dc, "摸键学徒", cx + 16.0, cy2 + 14.0, 22.0, C_TEXT, true, scale);
    draw_text(dc, "Lv.14 / 100", cx + 150.0, cy2 + 18.0, 15.0, 0x0064B5F6, true, scale);
    draw_text(dc, "凡键境", cx + cw - 90.0, cy2 + 18.0, 16.0, C_TEXT_DIM, false, scale);
    progress(dc, cx + 16.0, cy2 + 54.0, cw - 32.0, 10.0, 0.045, scale);
    draw_text(
        dc,
        "60 / 1400 经验 · 还差 1340 升下一级 · 已写 9160 字",
        cx + 16.0,
        cy2 + 72.0,
        14.0,
        C_TEXT_SUB,
        false,
        scale,
    );

    // 年度热力图（53 周 × 7 天）
    let hy = cy2 + card_h2 + 14.0;
    draw_text(dc, "2026 年度击键", cx, hy, 20.0, C_TEXT, true, scale);
    let cell = 11.0f32;
    let g = 3.0f32;
    let hx = cx;
    let hy2 = hy + 28.0;
    let shades = [C_CARD2, 0x00BBF7D0, 0x004ADE80, 0x0022C55E, 0x0015803D];
    for week in 0..53i32 {
        for day in 0..7i32 {
            let seed = ((week * 7 + day) as f32 * 12.9898).sin().abs();
            let lvl = ((seed * 5.0) as usize).min(4);
            let x = hx + week as f32 * (cell + g);
            let y = hy2 + day as f32 * (cell + g);
            fill_r(dc, x, y, cell, cell, shades[lvl], 2.0, scale);
        }
    }
    // 图例
    let ly = hy2 + 7.0 * (cell + g) + 8.0;
    draw_text(dc, "少", cx, ly, 14.0, C_TEXT_SUB, false, scale);
    for i in 0..5 {
        fill_r(dc, cx + 26.0 + i as f32 * 18.0, ly + 2.0, 13.0, 13.0, shades[i], 2.0, scale);
    }
    draw_text(
        dc,
        "多　（<100 / <500 / <1500 / ≥1500 键）",
        cx + 26.0 + 5.0 * 18.0 + 6.0,
        ly,
        14.0,
        C_TEXT_SUB,
        false,
        scale,
    );
}

/// 界面风格（配色方案）—— 照搬自 color-schemes.slint：候选预览 + 配色列表。
unsafe fn draw_schemes(dc: HDC, cx: f32, cy: f32, cw: f32, scale: f32) {
    // 候选窗预览
    let pw = cw;
    let ph = 96.0f32;
    fill_r(dc, cx, cy, pw, ph, C_CARD, 8.0, scale);
    // 选中候选
    fill_r(dc, cx + 10.0, cy + 30.0, 90.0, 36.0, 0x00FA3A0A, 4.0, scale);
    draw_text_center(dc, "1. 候选", cx + 10.0, cy + 30.0, 90.0, 36.0, 14.0, 0x00FFFFFF, false, scale);
    draw_text(dc, "2. 候选", cx + 110.0, cy + 38.0, 14.0, C_TEXT, false, scale);
    draw_text(dc, "3. 候选", cx + 200.0, cy + 38.0, 14.0, C_TEXT, false, scale);
    draw_text(dc, "备注", cx + 290.0, cy + 38.0, 13.0, C_TEXT_SUB, false, scale);

    draw_text(
        dc,
        "37 套配色 · 当前 碧水／Aqua",
        cx,
        cy + ph + 10.0,
        14.0,
        C_TEXT_DIM,
        false,
        scale,
    );

    // 配色列表
    let list_y = cy + ph + 38.0;
    let list_h = 825.0 - list_y - 20.0;
    let schemes = [
        ("碧水／Aqua", "Rime-Next", true, true),
        ("墨白／Ink", "Rime-Next", false, false),
        ("微软雅黑／dark", "Rime-Next", false, false),
        ("小狼毫默认", "Rime-Next", false, false),
        ("简静／Quiet", "yuhao", false, false),
        ("落雪／Snow", "yuhao", false, true),
        ("青松／Pine", "yuhao", false, false),
        ("朱砂／Cinnabar", "yuhao", false, false),
        ("繁星／Starry", "lain", false, false),
    ];
    let row_h = (list_h) / schemes.len() as f32;
    for (i, (name, author, active, modified)) in schemes.iter().enumerate() {
        let y = list_y + i as f32 * row_h;
        if *active {
            fill_r(dc, cx, y, cw, row_h - 4.0, C_NAV_SEL, 4.0, scale);
        }
        // 当前生效标记
        fill(dc, cx + 6.0, y + row_h / 2.0 - 8.0, 3.0, 16.0, if *active { 0x0087CEEB } else { C_LINE }, scale);
        draw_text(dc, name, cx + 18.0, y + 8.0, 16.0, C_TEXT, false, scale);
        draw_text(dc, *author, cx + 18.0, y + 30.0, 12.0, C_TEXT_SUB, false, scale);
        if *modified {
            fill_r(dc, cx + 140.0, y + row_h / 2.0 - 3.0, 6.0, 6.0, 0x0087CEEB, 3.0, scale);
        }
    }
}

/// 方案选单 —— 照搬自 schemas.slint：系列导航 + 可勾选方案树。
unsafe fn draw_schemas(dc: HDC, cx: f32, cy: f32, cw: f32, scale: f32) {
    draw_text(
        dc,
        "21 个可用方案　勾选要出现在方案选单里的方案（第一条是默认方案）",
        cx,
        cy,
        13.0,
        C_TEXT_DIM,
        false,
        scale,
    );
    // 动作按钮
    button(dc, cx + cw - 300.0, cy + 18.0, 90.0, 28.0, "重新读取", C_CARD2, C_TEXT_DIM, scale);
    button(dc, cx + cw - 200.0, cy + 18.0, 110.0, 28.0, "官方对话框…", C_CARD2, C_TEXT_DIM, scale);
    button(dc, cx + cw - 84.0, cy + 18.0, 84.0, 28.0, "保存并部署", 0x0064B5F6, C_TEXT, scale);

    let list_y = cy + 56.0;
    let list_h = 825.0 - list_y - 20.0;
    fill_r(dc, cx, list_y, cw, list_h, C_CARD, 8.0, scale);

    let rows: [(&str, &str, &str, bool, bool, bool, u32); 10] = [
        // name, id/author, desc, selected, header, active, indent-level-ish
        ("yuhao", "", "", false, true, false, 0),
        ("小鹤音形", "xhup", "声笔系列", true, false, false, 1),
        ("落雪拼音", "luxue_pinyin", "yuhao", true, false, true, 1),
        ("简静拼音", "jing_pinyin", "yuhao", false, false, false, 1),
        ("Rime 默认", "", "", false, true, false, 0),
        ("朙月拼音", "luna_pinyin", "Rime", true, false, true, 1),
        ("地球拼音", "terra_pinyin", "Rime", false, false, false, 1),
        ("双拼方案", "", "", false, true, false, 0),
        ("自然码双拼", "ziranma", "Rime", true, false, false, 1),
        ("小鹤双拼", "flypy", "Rime", true, false, false, 1),
    ];
    let row_h = (list_h - 16.0) / rows.len() as f32;
    for (i, (name, meta, desc, selected, header, active, _lvl)) in rows.iter().enumerate() {
        let y = list_y + 8.0 + i as f32 * row_h;
        if *header {
            draw_text(dc, name, cx + 14.0, y + 6.0, 15.0, C_TEXT, true, scale);
        } else {
            let indent = 18.0f32;
            checkbox(dc, cx + 14.0 + indent, y + row_h / 2.0 - 9.0, 18.0, *selected, scale);
            draw_text(dc, name, cx + 40.0 + indent, y + 6.0, 15.0, C_TEXT, false, scale);
            draw_text(dc, meta, cx + 40.0 + indent, y + 28.0, 12.0, C_TEXT_SUB, false, scale);
            draw_text(dc, desc, cx + 260.0 + indent, y + 12.0, 12.0, C_TEXT_SUB, false, scale);
            if *active {
                fill_r(dc, cx + cw - 80.0, y + row_h / 2.0 - 11.0, 56.0, 22.0, 0x002D6A31, 11.0, scale);
                draw_text_center(dc, "使用中", cx + cw - 80.0, y + row_h / 2.0 - 11.0, 56.0, 22.0, 12.0, 0x004ADE80, true, scale);
            }
        }
    }
}

/// 常规（设置表单）—— 照搬自 main.slint 的字段表单：分节 + 字段行。
unsafe fn draw_general(dc: HDC, cx: f32, cy: f32, cw: f32, scale: f32) {
    draw_text(dc, "常规", cx, cy, 20.0, C_TEXT, true, scale);
    let fields: [(&str, &str, &str, bool); 7] = [
        ("显示语言", "ui.language", "界面文字语言", false),
        ("候选窗字体", "style.font_face", "候选窗使用的字体", false),
        ("候选字号", "style.font_point", "候选文字大小", false),
        ("横向候选", "style.horizontal", "候选横向排列", true),
        ("显示编码", "style.inline_preedit", "直接显示编码", true),
        ("同步目录", "sync_dir", "用户数据同步目录", false),
        ("部署快捷键", "hotkeys.deploy", "重新部署的快捷键", false),
    ];
    let top = cy + 36.0;
    let row_h = 60.0f32;
    for (i, (title, key, desc, boolean)) in fields.iter().enumerate() {
        let y = top + i as f32 * (row_h + 4.0);
        fill_r(dc, cx, y, cw, row_h, C_CARD, 8.0, scale);
        draw_text(dc, title, cx + 16.0, y + 10.0, 17.0, C_TEXT, true, scale);
        draw_text(dc, key, cx + 16.0, y + 34.0, 12.0, C_TEXT_SUB, false, scale);
        draw_text(dc, desc, cx + 220.0, y + 22.0, 13.0, C_TEXT_DIM, false, scale);
        if *boolean {
            toggle(dc, cx + cw - 70.0, y + row_h / 2.0 - 12.0, 44.0, 24.0, true, scale);
        } else {
            fill_r(dc, cx + cw - 200.0, y + row_h / 2.0 - 12.0, 180.0, 24.0, C_CARD2, 6.0, scale);
            draw_text(dc, "点击选择", cx + cw - 190.0, y + row_h / 2.0 - 8.0, 13.0, C_TEXT_SUB, false, scale);
        }
    }
}

/// 更新 —— 照搬自 main.slint 的「自动检查更新」卡片。
unsafe fn draw_update(dc: HDC, cx: f32, cy: f32, cw: f32, scale: f32) {
    draw_text(dc, "其他", cx, cy, 20.0, C_TEXT, true, scale);
    let y = cy + 36.0;
    let h = 80.0f32;
    fill_r(dc, cx, y, cw, h, C_CARD, 8.0, scale);
    draw_text(dc, "自动检查更新", cx + 16.0, y + 14.0, 17.0, C_TEXT, true, scale);
    draw_text(dc, "WinSparkle · 当前用户", cx + 16.0, y + 40.0, 12.0, C_TEXT_SUB, false, scale);
    draw_text(dc, "定期检查新版本。", cx + 16.0, y + 56.0, 12.0, C_TEXT_DIM, false, scale);
    toggle(dc, cx + cw - 70.0, y + h / 2.0 - 12.0, 44.0, 24.0, true, scale);
}

/// 词库 —— 照搬自 dicts.slint：查编码 / 加词 / 删词 + 结果列表。
unsafe fn draw_dict(dc: HDC, cx: f32, cy: f32, cw: f32, scale: f32) {
    draw_text(dc, "词库", cx, cy, 20.0, C_TEXT, true, scale);
    draw_text(dc, "用户目录 *.dict.yaml 汇总", cx + 60.0, cy + 6.0, 13.0, C_TEXT_DIM, false, scale);

    // 查编码
    input(dc, cx, cy + 36.0, cw - 90.0, 32.0, "编码（如 cs，可只打前缀）", scale);
    button(dc, cx + cw - 80.0, cy + 36.0, 80.0, 32.0, "查候选", C_CARD2, C_TEXT, scale);
    // 加 / 删
    input(dc, cx, cy + 76.0, cw - 200.0, 32.0, "要加的词", scale);
    button(dc, cx + cw - 190.0, cy + 76.0, 90.0, 32.0, "加入", C_CARD2, C_TEXT, scale);
    button(dc, cx + cw - 90.0, cy + 76.0, 90.0, 32.0, "删除该词", C_CARD2, C_TEXT, scale);

    // 结果区
    let list_y = cy + 120.0;
    let list_h = 825.0 - list_y - 20.0;
    fill_r(dc, cx, list_y, cw, list_h, C_CARD, 8.0, scale);
    let rows = [
        ("1", "的", "de", "用户词典"),
        ("2", "是", "shi", "系统词典"),
        ("3", "我", "wo", "用户词典"),
        ("4", "一", "yi", "系统词典"),
        ("5", "了", "le", "系统词典"),
    ];
    let rh = 34.0f32;
    // 表头
    draw_text(dc, "#", cx + 14.0, list_y + 8.0, 13.0, C_TEXT_SUB, false, scale);
    draw_text(dc, "词语", cx + 60.0, list_y + 8.0, 14.0, C_TEXT_DIM, false, scale);
    draw_text(dc, "编码", cx + 260.0, list_y + 8.0, 13.0, C_TEXT_SUB, false, scale);
    draw_text(dc, "来源", cx + 400.0, list_y + 8.0, 13.0, C_TEXT_SUB, false, scale);
    for (i, (rank, word, code, src)) in rows.iter().enumerate() {
        let y = list_y + 28.0 + i as f32 * rh;
        fill(dc, cx, y, cw, 1.0, C_LINE, scale);
        draw_text(dc, rank, cx + 14.0, y + 8.0, 13.0, C_TEXT_SUB, false, scale);
        draw_text(dc, word, cx + 60.0, y + 6.0, 16.0, C_TEXT, false, scale);
        draw_text(dc, code, cx + 260.0, y + 8.0, 13.0, 0x004ADE80, false, scale);
        draw_text(dc, src, cx + 400.0, y + 8.0, 12.0, C_TEXT_SUB, false, scale);
    }
}

/// 下载（方案市场）—— 照搬自 download.slint：分类页签 + 仓库卡片列表。
unsafe fn draw_download(dc: HDC, cx: f32, cy: f32, cw: f32, scale: f32) {
    draw_text(dc, "下载", cx, cy, 20.0, C_TEXT, true, scale);
    draw_text(dc, "浏览 Rime-Next 组织并一键安装", cx + 60.0, cy + 6.0, 13.0, C_TEXT_DIM, false, scale);

    // 分类页签
    let cats = ["全部", "方案", "词典", "皮肤", "插件"];
    let tab_w = 70.0f32;
    for (i, cat) in cats.iter().enumerate() {
        let x = cx + i as f32 * (tab_w + 6.0);
        let sel = i == 0;
        fill_r(dc, x, cy + 34.0, tab_w, 28.0, if sel { 0x00185FA5 } else { C_NAV }, 6.0, scale);
        draw_text_center(dc, cat, x, cy + 34.0, tab_w, 28.0, 13.0, if sel { 0x00FFFFFF } else { C_TEXT_DIM }, false, scale);
    }

    // 仓库列表
    let list_y = cy + 76.0;
    let list_h = 825.0 - list_y - 20.0;
    fill_r(dc, cx, list_y, cw, list_h, C_CARD, 8.0, scale);
    let repos = [
        ("librime-next", "Rime 输入法引擎下一代", "方案", false),
        ("weasel-next", "小狼毫 Windows 前端", "方案", true),
        ("pinyin-dict", "拼音词库大全", "词典", false),
        ("schemers", "社区配色与皮肤合集", "皮肤", false),
        ("rime-aca", "成语接龙插件", "插件", false),
    ];
    let rh = (list_h - 16.0) / repos.len() as f32;
    for (i, (name, desc, cat, installed)) in repos.iter().enumerate() {
        let y = list_y + 8.0 + i as f32 * rh;
        fill(dc, cx, y, cw, 1.0, C_LINE, scale);
        // 头像
        let initial = &name[0..1];
        fill_r(dc, cx + 12.0, y + rh / 2.0 - 15.0, 30.0, 30.0, 0x00378ADD, 15.0, scale);
        draw_text_center(dc, initial, cx + 12.0, y + rh / 2.0 - 15.0, 30.0, 30.0, 15.0, 0x00FFFFFF, true, scale);
        draw_text(dc, name, cx + 54.0, y + 10.0, 15.0, C_TEXT, true, scale);
        draw_text(dc, desc, cx + 54.0, y + 32.0, 12.0, C_TEXT_DIM, false, scale);
        draw_text(dc, cat, cx + 280.0, y + 14.0, 12.0, C_TEXT_SUB, false, scale);
        let btn_label = if *installed { "已装" } else { "安装" };
        button(dc, cx + cw - 84.0, y + rh / 2.0 - 15.0, 72.0, 30.0, btn_label, C_CARD2, C_TEXT, scale);
    }
}
