//! Vulkan 版候选窗 · **挤压式三模式**(2026-11-03,照用户 HTML 设计稿):
//! 横流(单行)/ 飞瀑(竖列)/ 竹简(格子,内容顶齐向下长)。
//!
//! 设计要点(HTML 规格逐条):
//! * 灰壳 #9a9a9f(激活 #6f6f76)padding 0.5em 圆角 1em;内板 #f5f5f7;
//! * **红色 0.5em 方块**做分隔(项间/按钮间/序号与文字间);
//! * **绿底黑线**分隔(vsep 竖 0.5em×1em / hsep 横整宽×0.5em,中线 2px);
//! * **字符格子排版**:CJK 1em / ASCII 0.5em,每字符居中在自己的格子;
//! * **挤压选中**:选中项 padding 0.25em→0.5em(宽高各 +0.5em),邻居被
//!   真实挤开滑动让位;缓动 = VSCode cubic-bezier(0.16,1,0.3,1)(easeOutExpo 近似);
//! * 箭头键移动选中(←↑ = 前一个,→↓ = 后一个);
//! * **帧计时**:GDI 绘制 / 降采样 / 上传 / 提交 / 呈现 分段计时,
//!   退出时打印 P50/P95——与 D2D 主题(theme.log 逐帧 t)对比。
//!
//! 管线:GDI+CPU 混合把内容画进超采样 DIB → 降采样+预乘 alpha →
//! 上传 VkImage → `vkCmdCopyImage` 贴到交换链(PRE_MULTIPLIED 透明)。
//!
//! 抗锯齿策略：以 `SS` 倍超采样绘制整张位图（文字灰度 AA + 圆角 SDF 在超采样
//! 分辨率下自然平滑），再用盒式平均降采样压回 1× 上传 —— 等价于硬件 SSAA。
//! 渲染目标分辨率严格对齐交换链 `current_extent`（HiDPI 下为物理像素），避免错位。
//!
//! 布局/尺寸严格对齐 `cand_preview.html` 的规格（字高=基准，其余由它推导）：
//!   --base=20(字高) --pad=0.5em(小框/行距) --gap=1em(大框)
//!   --item-gap=0.5em(水平)/1em(垂直) --radius-inner=0.5em --radius-outer=1em
//!
//! 透明（B）：窗口加 `WS_EX_NOREDIRECTIONBITMAP`，交换链用预乘 alpha；圆角外/卡片间
//! 缝隙的像素 alpha=0 直接透出桌面，圆角边缘由 SDF 覆盖率做平滑抗锯齿。淡入/淡出
//! 通过整体缩放 alpha 实现（不依赖 GDI 硬裁剪）。
//!
//! 动画（C）：
//!   * 出现滑入：窗口从下方 SLIDE_DIST 滑入目标位（ease-out cubic）
//!   * 高亮块平滑滑动：自动在候选间循环「选词」，高亮块按 dt 指数平滑滑动
//!   * 翻页循环演示：每 PAGE_PERIOD 翻一页，旧页滑出、新页滑入（水平左/右，垂直上/下）

use std::sync::atomic::Ordering;

use ash::{khr, vk, Entry};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject,
    FillRect, GetTextExtentExPointW, IntersectClipRect, SelectClipRgn, SelectObject, SetBkMode,
    SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, FONT_CHARSET,
    FONT_CLIP_PRECISION, FONT_OUTPUT_PRECISION, FONT_QUALITY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow, CS_HREDRAW,
    CS_VREDRAW, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW, WNDCLASSW, WS_POPUP, WS_VISIBLE,
};

/// 超采样倍率：文字与圆角都按此倍数绘制后再降采样，是消除锯齿的关键
const SS: u32 = 3;

/// 每页候选数 / 页数（演示数据 10 个候选 → 两页）
const PAGE_SIZE: usize = 5;
const PAGES: usize = 2;

/// 动画时长（秒）
const FADE_IN: f32 = 0.30; // 淡入
const FADE_OUT: f32 = 0.22; // 淡出（Esc 后）
const SLIDE_DIST: f32 = 40.0; // 出现滑入距离（像素）
/// 挤压过渡时长(VSCode 缓动 250ms)
const SQUEEZE_MS: f32 = 0.25;
/// 入场 riseIn:单项 250ms,错峰 30ms
const RISE_MS: f32 = 0.25;
const RISE_STAGGER: f32 = 0.03;

/// VSCode 缓动 cubic-bezier(0.16,1,0.3,1) ≈ easeOutExpo
fn vscode_ease(t: f32) -> f32 {
    if t >= 1.0 { 1.0 } else { 1.0 - 2.0f32.powf(-10.0 * t) }
}

/// 字符宽分类:CJK 全角 1em,ASCII/半角 0.5em(HTML isWide 同款分段)
fn is_wide(c: char) -> bool {
    let c = c as u32;
    (0x2E80..=0x9FFF).contains(&c)
        || (0x3000..=0x303F).contains(&c)
        || (0xFF00..=0xFF60).contains(&c)
        || (0x25B0..=0x25C0).contains(&c)
        || (0x2600..=0x27BF).contains(&c)
        || c >= 0x1F000
}

/// 窗口逻辑位置
const BASE_X: i32 = 300;
const BASE_Y: i32 = 260;

/// 圆角矩形有符号距离场（负=内部，正=外部）
#[inline]
fn sdf_rr(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let qx = (px - (x0 + x1) / 2.0).abs() - ((x1 - x0) / 2.0 - r);
    let qy = (py - (y0 + y1) / 2.0).abs() - ((y1 - y0) / 2.0 - r);
    let ax = qx.max(0.0);
    let ay = qy.max(0.0);
    let outside = (ax * ax + ay * ay).sqrt();
    let inside = qx.min(0.0).max(qy.min(0.0));
    outside + inside - r
}

/// 圆角矩形覆盖率（2×2 超采样抗锯齿），返回 0..1（用于高亮小框）
fn sdf_round(px: f32, py: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let mut sum = 0.0f32;
    for sy in 0..2 {
        for sx in 0..2 {
            let qx = px + (sx as f32 - 0.5) * 0.5;
            let qy = py + (sy as f32 - 0.5) * 0.5;
            let dx = if qx < x0 + r {
                x0 + r - qx
            } else if qx > x1 - r {
                qx - (x1 - r)
            } else {
                0.0
            };
            let dy = if qy < y0 + r {
                y0 + r - qy
            } else if qy > y1 - r {
                qy - (y1 - r)
            } else {
                0.0
            };
            sum += if dx > 0.0 && dy > 0.0 {
                (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
    }
    sum / 4.0
}

pub unsafe fn run(entry: &Entry) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let base = 20.0f32; // 1em = 字高
        // 挤压两态 padding:普通 0.25em,选中 0.5em(动画插值)
        const PAD_N: f32 = 0.25;
        const PAD_S: f32 = 0.5;
        // 候选(HTML 三模式共用一份序号+文本;竹简用加长文本)
        let items_row: [(&str, &str); 5] = [
            ("1", "你好"), ("2", "您"), ("3", "泥"), ("4", "拟"), ("5", "逆"),
        ];
        let items_cells: [(&str, &str); 5] = [
            ("1", "你好"), ("2", "你好吗"), ("3", "你家"), ("4", "拟好了没"), ("5", "泥石流"),
        ];
        let preedit = "ni'hao";
        let btns = ["🔣", "✂️", "🔄"];

        // 字格宽:run 的 em 宽度(每字符 宽1/窄0.5)
        let run_w = |text: &str| -> f32 {
            text.chars()
                .map(|c| if is_wide(c) { 1.0 } else { 0.5 })
                .sum::<f32>()
                * base
        };
        let item_core_w = |num: &str, text: &str| -> f32 {
            run_w(num) + 0.5 * base + run_w(text) // 序号格 + 红块 0.5em + 文本
        };

        // 每帧布局:给定动画状态(选中下标 + 插值),算三壳的几何。
        // 返回 (shell_rects[3], 各壳内容绘制所需的槽位)。
        // 简化:壳宽取"选中态"固定值(动画只动内容),壳位置固定堆叠。
        let layout_shell_w = |items: &[(&str, &str)]| -> f32 {
            // 选中态壳宽(全部按 0.5em pad 算最大值)
            let cand_w: f32 = items
                .iter()
                .map(|(num, text)| item_core_w(num, text) + 2.0 * PAD_S)
                .sum::<f32>()
                + 0.5 * base * (items.len() as f32 - 1.0); // 项间红块
            let bar_w = run_w(preedit)
                + 0.5 * base // vsep
                + (btns.len() as f32) * base
                + 0.5 * base * (btns.len() as f32 - 1.0)
                + 0.5 * base // vsep
                + run_w("◀") + 0.5 * base + run_w("▶");
            let content = bar_w.max(cand_w);
            content + 2.0 * 0.5 * base // 壳 padding 0.5em
        };
        let shell_w = [
            layout_shell_w(&items_row),
            layout_shell_w(&items_row),
            layout_shell_w(&items_cells),
        ];
        // 壳高:横流 = bar 1.6em + hsep 0.5em + 行 2.5em + 壳 padding 1em
        let shell_h = [
            1.6 * base + 0.5 * base + 2.5 * base + 1.0 * base,
            1.6 * base + 0.5 * base + (base + 2.0 * PAD_S) * 5.0 + 0.5 * base * 4.0 + 1.0 * base,
            1.6 * base + 0.5 * base + 7.0 * base + 1.0 * base,
        ];
        let mode_gap = 1.0 * base;
        let margin = 0.5 * base;
        // 窗口逻辑尺寸 + 三壳矩形(堆叠,左对齐)
        let total_w = shell_w.iter().cloned().fold(0.0f32, f32::max) + 2.0 * margin;
        let total_h = shell_h.iter().sum::<f32>() + mode_gap * 2.0 + 2.0 * margin;
        let shell_x = [margin, margin, margin];
        let shell_y = [
            margin,
            margin + shell_h[0] + mode_gap,
            margin + shell_h[0] + shell_h[1] + mode_gap * 2.0,
        ];

        // ---- 窗口 ----
        // WS_EX_NOREDIRECTIONBITMAP(0x00200000)：让 DWM 直接合成交换链（含 alpha），
        // 不分配 GDI 重定向位图 —— 这是 Vulkan/DXGI flip 模型下做真正透明的关键。
        // 置顶(0x8) + 工具窗(0x80)：不进任务栏、不被其他窗口压住。
        let ex_style = windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(
            0x0020_0000 | 0x0000_0008 | 0x0000_0080,
        );
        let module = GetModuleHandleW(None)?;
        let class_name = w!("VkCandidateWindow");
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(crate::wnd_proc),
            hInstance: module.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wnd_class);
        let hwnd = CreateWindowExW(
            ex_style,
            class_name,
            w!("候选窗 (Vulkan)"),
            WS_POPUP | WS_VISIBLE,
            BASE_X,
            BASE_Y,
            total_w as i32,
            total_h as i32,
            None,
            None,
            Some(module.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);

        // ---- Vulkan 初始化，先拿到交换链真实分辨率 ----
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0));
        let instance = entry.create_instance(
            &vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_extension_names(&[
                    khr::surface::NAME.as_ptr(),
                    khr::win32_surface::NAME.as_ptr(),
                ]),
            None,
        )?;
        let win32_fn = khr::win32_surface::Instance::new(entry, &instance);
        let surface = win32_fn.create_win32_surface(
            &vk::Win32SurfaceCreateInfoKHR::default()
                .hinstance(module.0 as isize)
                .hwnd(hwnd.0 as isize),
            None,
        )?;
        let surface_fn = khr::surface::Instance::new(entry, &instance);

        let physical = {
            let mut found = None;
            for device in instance.enumerate_physical_devices()? {
                let props = instance.get_physical_device_properties(device);
                let families = instance.get_physical_device_queue_family_properties(device);
                let graphics = families
                    .iter()
                    .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS));
                let present = families.iter().enumerate().position(|(index, _)| {
                    surface_fn
                        .get_physical_device_surface_support(device, index as u32, surface)
                        .unwrap_or(false)
                });
                if graphics.is_some() && present.is_some() {
                    if props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU {
                        found = Some(device);
                        break;
                    }
                    if found.is_none() {
                        found = Some(device);
                    }
                }
            }
            found.ok_or("没有可用设备")?
        };
        let families = instance.get_physical_device_queue_family_properties(physical);
        let graphics_family = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .unwrap() as u32;
        let priorities = [1.0f32];
        let device = instance.create_device(
            physical,
            &vk::DeviceCreateInfo::default()
                .queue_create_infos(&[vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(graphics_family)
                    .queue_priorities(&priorities)])
                .enabled_extension_names(&[khr::swapchain::NAME.as_ptr()]),
            None,
        )?;
        let queue = device.get_device_queue(graphics_family, 0);
        let swapchain_fn = khr::swapchain::Device::new(&instance, &device);

        let caps = surface_fn.get_physical_device_surface_capabilities(physical, surface)?;
        let formats = surface_fn.get_physical_device_surface_formats(physical, surface)?;
        let format = formats
            .iter()
            .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
            .copied()
            .unwrap_or(formats[0]);
        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: total_w as u32,
                height: total_h as u32,
            }
        };
        // 把 logical 布局整体缩放到交换链分辨率，再叠加 SS 超采样
        let scale = extent.width as f32 / total_w;
        let k = scale * SS as f32;
        let width = extent.width as i32;
        let height = extent.height as i32;
        let dib_w = (total_w * k) as i32;
        let dib_h = (total_h * k) as i32;

        // 交换链合成方式：优先预乘 alpha（透明）；不支持则退回不透明（硬裁剪兜底）。
        let transparent = caps
            .supported_composite_alpha
            .contains(vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED);
        let swapchain = swapchain_fn.create_swapchain(
            &vk::SwapchainCreateInfoKHR::default()
                .surface(surface)
                .min_image_count((caps.min_image_count + 1).min(3))
                .image_format(format.format)
                .image_color_space(format.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(
                    vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST,
                )
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(if transparent {
                    vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED
                } else {
                    vk::CompositeAlphaFlagsKHR::OPAQUE
                })
                .present_mode(vk::PresentModeKHR::FIFO)
                .clipped(true),
            None,
        )?;
        let images = swapchain_fn.get_swapchain_images(swapchain)?;

        // 覆盖率场:三个灰壳的圆角矩形(圆角外 alpha=0)。超采样分辨率算一次。
        let soft = SS as f32;
        let mut cov_ss = vec![0.0f32; (dib_w * dib_h) as usize];
        {
            let rk = 1.0 * base * k; // 壳圆角 1em
            for py in 0..dib_h {
                for px in 0..dib_w {
                    let fx = px as f32 + 0.5;
                    let fy = py as f32 + 0.5;
                    let mut sd = f32::MAX;
                    for i in 0..3 {
                        let sd_i = sdf_rr(
                            fx,
                            fy,
                            shell_x[i] * k,
                            shell_y[i] * k,
                            (shell_x[i] + shell_w[i]) * k,
                            (shell_y[i] + shell_h[i]) * k,
                            rk,
                        );
                        sd = sd.min(sd_i);
                    }
                    cov_ss[(py * dib_w + px) as usize] = (0.5 - sd / soft).clamp(0.0, 1.0);
                }
            }
        }

        // 兜底：不透明模式下用窗口物理裁剪维持圆角形状(三壳并集)
        if !transparent {
            use windows::Win32::Graphics::Gdi::{
                CombineRgn, CreateRectRgn, CreateRoundRectRgn, RGN_OR, SetWindowRgn,
            };
            let s = scale;
            let r = (1.0 * base * s) as i32;
            let hrgn = CreateRectRgn(0, 0, 0, 0);
            for i in 0..3 {
                let hrgn_i = CreateRoundRectRgn(
                    (shell_x[i] * s) as i32,
                    (shell_y[i] * s) as i32,
                    ((shell_x[i] + shell_w[i]) * s) as i32,
                    ((shell_y[i] + shell_h[i]) * s) as i32,
                    r,
                    r,
                );
                CombineRgn(Some(hrgn), Some(hrgn), Some(hrgn_i), RGN_OR);
                let _ = DeleteObject(hrgn_i.into());
            }
            SetWindowRgn(hwnd, Some(hrgn), true);
            let _ = DeleteObject(hrgn.into());
        }

        // ---- DIB（超采样位图，每帧重绘）----
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: dib_w,
                biHeight: -dib_h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let old_bitmap = SelectObject(dc, bitmap.into());
        let _ = SetBkMode(dc, TRANSPARENT);

        // ── 调色板(HTML 设计稿同款;COLORREF = 0x00BBGGRR)──
        let col_shell: u32 = 0x009F9A9A; // #9a9a9f 灰壳
        let col_inner: u32 = 0x00F7F5F5; // #f5f5f7 白内板/核
        let col_red: u32 = 0x000000FF; // #ff0000 红方块
        let col_green: u32 = 0x00008000; // #008000 绿分隔
        let col_pinyin: u32 = 0x00555555; // #555 拼音
        let col_num: u32 = 0x00999999; // #999 序号
        let col_text: u32 = 0x00222222; // #222 候选文本
        let col_page: u32 = 0x00FE5A3D; // #3d5afe 翻页箭头(COLORREF 反序)
        let col_orange: u32 = 0x0000A5FF; // #ffa500 候选底
        let col_orange_hover: u32 = 0x000070CC; // #cc7000

        let draw_text = |x: f32, y: f32, content: &str, color: u32| {
            let font = CreateFontW(
                (base * k) as i32,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                FONT_CHARSET(1),
                FONT_OUTPUT_PRECISION(0),
                FONT_CLIP_PRECISION(0),
                FONT_QUALITY(4),
                0,
                w!("Microsoft YaHei UI"),
            );
            let old = SelectObject(dc, font.into());
            let _ = SetTextColor(dc, COLORREF(color));
            let wide: Vec<u16> = content.encode_utf16().collect();
            let _ = TextOutW(dc, (x * k) as i32, (y * k) as i32, &wide);
            let _ = SelectObject(dc, old);
            let _ = DeleteObject(font.into());
        };
        let fill_rect = |x0: f32, y0: f32, x1: f32, y1: f32, color: u32| {
            let brush = CreateSolidBrush(COLORREF(color));
            let rect = RECT {
                left: (x0 * k) as i32,
                top: (y0 * k) as i32,
                right: (x1 * k) as i32,
                bottom: (y1 * k) as i32,
            };
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(brush.into());
        };
        let paint_rounded = |x0: f32, y0: f32, x1: f32, y1: f32, radius: f32, (cr, cg, cb): (u8, u8, u8)| {
            let (x0, y0, x1, y1) = (
                (x0 * k) as i32,
                (y0 * k) as i32,
                (x1 * k) as i32,
                (y1 * k) as i32,
            );
            let r = radius * k;
            for py in y0.max(0)..y1.min(dib_h) {
                for px in x0.max(0)..x1.min(dib_w) {
                    let cov = sdf_round(
                        px as f32 + 0.5,
                        py as f32 + 0.5,
                        x0 as f32,
                        y0 as f32,
                        x1 as f32,
                        y1 as f32,
                        r,
                    );
                    if cov <= 0.0 {
                        continue;
                    }
                    let off = ((py * dib_w + px) * 4) as usize;
                    let dst = (bits as *mut u8).add(off);
                    let blend = |o: u8, n: u8| ((o as f32 * (1.0 - cov) + n as f32 * cov) as u8);
                    unsafe {
                        *dst = blend(*dst, cb);
                        *dst.add(1) = blend(*dst.add(1), cg);
                        *dst.add(2) = blend(*dst.add(2), cr);
                    }
                }
            }
        };

        // ── 挤压式布局 + 绘制 ─────────────────────────────────────────
        // (状态变量在帧循环里声明——draw_shell 闭包每帧以参数收)
        let draw_run = |x: f32, y: f32, text: &str, color: u32| {
            let mut cx = x;
            for ch in text.chars() {
                let w = if is_wide(ch) { 1.0 } else { 0.5 } * base;
                draw_text(cx + (w - base) / 2.0, y, &ch.to_string(), color);
                cx += w;
            }
        };
        let red_block = |x: f32, y: f32| {
            fill_rect(x, y, x + 0.5 * base, y + 0.5 * base, col_red);
        };
        let vsep = |x: f32, y: f32| {
            fill_rect(x, y, x + 0.5 * base, y + base, col_green);
            fill_rect(x + 0.5 * base / 2.0 - 1.0 * k, y, x + 0.5 * base / 2.0 + 1.0 * k, y + base, 0x00000000);
        };
        let hsep = |x: f32, y: f32, w: f32| {
            fill_rect(x, y, x + w, y + 0.5 * base, col_green);
            fill_rect(x, y + 0.5 * base / 2.0 - 1.0 * k, x + w, y + 0.5 * base / 2.0 + 1.0 * k, 0x00000000);
        };

        // 画单壳(mode: 0=横流 1=飞瀑 2=竹简)
        // pad_of: 动画态 padding(选中渐大,旧选中渐收)
        let draw_shell = |mode: usize, ox: f32, oy: f32, shell_w: f32, shell_h: f32, pad_s: f32, sel: usize, old_sel: usize, rise: f32| {
            let b = base;
            let pad_of = |i: usize| -> f32 {
                if i == sel { PAD_N * b + (PAD_S - PAD_N) * b * pad_s }
                else if i == old_sel { PAD_S * b - (PAD_S - PAD_N) * b * pad_s }
                else { PAD_N * b }
            };
            let items = if mode == 2 { &items_cells } else { &items_row };
            paint_rounded(ox, oy, ox + shell_w, oy + shell_h, 1.0 * b, (0x9a, 0x9a, 0x9f));
            let in_x = ox + 0.5 * b;
            let in_y = oy + 0.5 * b;
            let in_w = shell_w - 1.0 * b;
            paint_rounded(in_x, in_y, in_x + in_w, oy + shell_h - 0.5 * b, 0.25 * b, (0xf5, 0xf5, 0xf7));
            let bar_y = in_y + 0.3 * b;
            draw_run(in_x + 0.3 * b, bar_y, preedit, 0x00555555);
            let mut x = in_x + 0.3 * b + run_w(preedit) + 0.3 * b;
            fill_rect(x, bar_y, x + 0.5 * b, bar_y + b, 0x00008000);
            fill_rect(x + 0.25 * b * 0.5, bar_y, x + 0.25 * b * 0.5 + 2.0 * k, bar_y + b, 0x00000000);
            x += 0.5 * b + 0.2 * b;
            for (bi, btn) in btns.iter().enumerate() {
                draw_run(x, bar_y, btn, 0x00999999);
                x += base;
                if bi + 1 < btns.len() {
                    fill_rect(x, bar_y + 0.25 * b, x + 0.5 * b, bar_y + 0.75 * b, 0x000000FF);
                    x += 0.5 * b;
                }
            }
            x += 0.3 * b;
            fill_rect(x, bar_y, x + 0.5 * b, bar_y + b, 0x00008000);
            fill_rect(x + 0.25 * b * 0.5, bar_y, x + 0.25 * b * 0.5 + 2.0 * k, bar_y + b, 0x00000000);
            x += 0.5 * b + 0.2 * b;
            draw_run(x, bar_y, "◀", 0x00FE5A3D);
            fill_rect(x + base, bar_y + 0.25 * b, x + base + 0.5 * b, bar_y + 0.75 * b, 0x000000FF);
            draw_run(x + base + 0.5 * b, bar_y, "▶", 0x00FE5A3D);
            // hsep
            fill_rect(in_x, in_y + 1.6 * b, in_x + in_w, in_y + 1.6 * b + 0.5 * b, 0x00008000);
            fill_rect(in_x, in_y + 1.6 * b + 0.25 * b, in_x + in_w, in_y + 1.6 * b + 0.25 * b + 2.0 * k, 0x00000000);
            // 候选区
            let cand_y = in_y + 1.6 * b + 0.5 * b;
            let rise_off = |i: usize, now: f32| -> f32 {
                let st = RISE_STAGGER * i as f32;
                let local = ((now - st) / RISE_MS).clamp(0.0, 1.0);
                (1.0 - vscode_ease(local)) * 0.3 * b * (if now < st + RISE_MS { 1.0 } else { 0.0 })
            };
            let pad_of = |i: usize| -> f32 {
                if i == sel { PAD_N * b + (PAD_S - PAD_N) * b * pad_s }
                else if i == old_sel { PAD_S * b - (PAD_S - PAD_N) * b * pad_s }
                else { PAD_N * b }
            };
            let draw_item = |i: usize, it_x: f32, it_y: f32, it_h: f32| {
                let (num, text) = &items[i];
                let p = pad_of(i);
                let core_w = item_core_w(num, text);
                let bg = if i == sel { (0x00u8, 0x70u8, 0xccu8) } else { (0x00u8, 0xa5u8, 0xffu8) };
                paint_rounded(it_x, it_y, it_x + core_w + 2.0 * p, it_y + it_h, 0.5 * b, bg);
                let core_x = it_x + p;
                let core_y = it_y + p;
                paint_rounded(core_x, core_y, core_x + core_w, core_y + b, 0.25 * b, (0xf5, 0xf5, 0xf7));
                draw_run(core_x, core_y, num, 0x00999999);
                fill_rect(core_x + run_w(num), core_y + 0.25 * b, core_x + run_w(num) + 0.5 * b, core_y + 0.75 * b, 0x000000FF);
                draw_run(core_x + run_w(num) + 0.5 * b, core_y, text, 0x00222222);
            };
            match mode {
                0 => {
                    let mut x = in_x + 0.3 * b;
                    let row_cy = cand_y + 1.25 * b;
                    for (i, (num, text)) in items.iter().enumerate() {
                        let p = pad_of(i);
                        let core_w = item_core_w(num, text);
                        let it_h = b + 2.0 * p;
                        let it_y = row_cy - it_h / 2.0 + rise_off(i, rise);
                        draw_item(i, x, it_y, it_h);
                        x += core_w + 2.0 * p + 0.5 * b;
                        fill_rect(x - 0.5 * b, row_cy - 0.25 * b, x, row_cy + 0.25 * b, 0x000000FF);
                    }
                }
                1 => {
                    let mut y = cand_y;
                    for (i, (num, text)) in items.iter().enumerate() {
                        let p = pad_of(i);
                        let core_w = item_core_w(num, text);
                        let it_h = b + 2.0 * p;
                        let it_y = y + rise_off(i, rise);
                        draw_item(i, in_x + 0.3 * b, it_y, it_h);
                        y = it_y + it_h + 0.5 * b;
                        fill_rect(in_x + 0.3 * b, y - 0.25 * b, in_x + 0.3 * b + 0.5 * b, y, 0x000000FF);
                        y += 0.5 * b;
                    }
                }
                _ => {
                    let mut x = in_x + 0.3 * b;
                    let cell_top = cand_y;
                    for (i, (num, text)) in items.iter().enumerate() {
                        let p = pad_of(i);
                        let core_w = item_core_w(num, text);
                        let it_w = core_w + 2.0 * p;
                        let it_x = x + rise_off(i, rise);
                        draw_item(i, it_x, cell_top, 6.0 * b);
                        x = it_x + it_w + 0.5 * b;
                        fill_rect(x - 0.5 * b, cell_top + 0.25 * b, x, cell_top + 0.75 * b, 0x000000FF);
                    }
                }
            }
        };

        // 每帧布局:三壳(pad_s 挤压插值),返回壳矩形列表
        let layout_state = |pad_s: f32, sel: usize, old_sel: usize| -> Vec<(f32, f32, f32, f32)> {
            let b = base;
            let pad_of = |i: usize, sel: usize, old_sel: usize| -> f32 {
                if i == sel { PAD_N * b + (PAD_S - PAD_N) * b * pad_s }
                else if i == old_sel { PAD_S * b - (PAD_S - PAD_N) * b * pad_s }
                else { PAD_N * b }
            };
            let mut rects = Vec::new();
            let mut y = margin;
            for (mode, shell_w) in [(0usize, shell_w[0]), (1, shell_w[1]), (2, shell_w[2])] {
                let items = if mode == 2 { &items_cells } else { &items_row };
                let bar_w = run_w(preedit)
                    + 0.5 * b
                    + 3.0 * b + 0.5 * b * 2.0
                    + 0.5 * b
                    + run_w("◀") + 0.5 * b + run_w("▶");
                let cand_w: f32 = items
                    .iter()
                    .enumerate()
                    .map(|(i, (num, text))| item_core_w(num, text) + 2.0 * pad_of(i, sel, old_sel))
                    .sum::<f32>()
                    + 0.5 * b * (items.len() as f32 - 1.0);
                let content = bar_w.max(cand_w);
                let shell_w = content + 1.0 * b;
                rects.push((margin, y, margin + shell_w, y + shell_h[mode]));
                y += shell_h[mode] + mode_gap;
            }
            rects
        };

        // ---- 上传资源（创建一次，循环复用）----
        let ranges = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);
        let image = device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::B8G8R8A8_UNORM)
                .extent(vk::Extent3D {
                    width: width as u32,
                    height: height as u32,
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
                .sharing_mode(vk::SharingMode::EXCLUSIVE)
                .initial_layout(vk::ImageLayout::UNDEFINED),
            None,
        )?;
        let requirements = device.get_image_memory_requirements(image);
        let mem_props = instance.get_physical_device_memory_properties(physical);
        let memory_type = (0..mem_props.memory_type_count)
            .find(|index| requirements.memory_type_bits & (1 << index) != 0)
            .unwrap_or(0);
        let image_memory = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(requirements.size)
                .memory_type_index(memory_type),
            None,
        )?;
        device.bind_image_memory(image, image_memory, 0)?;

        let byte_len = (width as usize * height as usize * 4) as u64;
        let buffer = device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(byte_len)
                .usage(vk::BufferUsageFlags::TRANSFER_SRC),
            None,
        )?;
        let buffer_requirements = device.get_buffer_memory_requirements(buffer);
        let buffer_type = (0..mem_props.memory_type_count)
            .find(|index| {
                buffer_requirements.memory_type_bits & (1 << index) != 0
                    && mem_props.memory_types[*index as usize]
                        .property_flags
                        .contains(vk::MemoryPropertyFlags::HOST_VISIBLE)
            })
            .unwrap_or(0);
        let buffer_memory = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(buffer_requirements.size)
                .memory_type_index(buffer_type),
            None,
        )?;
        device.bind_buffer_memory(buffer, buffer_memory, 0)?;
        let mapped = device.map_memory(buffer_memory, 0, byte_len, vk::MemoryMapFlags::empty())?;

        let pool = device.create_command_pool(
            &vk::CommandPoolCreateInfo::default().queue_family_index(graphics_family),
            None,
        )?;
        let cmd = device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0];
        let sem_available = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
        let sem_rendered = device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?;
        let fence = device.create_fence(
            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
            None,
        )?;

        let mut pixels = vec![0u8; (width as usize * height as usize * 4) as usize];
        let n_ss = (SS * SS) as u32;

        // ---- 帧驱动渲染循环(挤压三模式 + 箭头选词 + 帧计时)----
        let start = std::time::Instant::now();
        let mut last = start;
        let mut closing_start: Option<std::time::Instant> = None;
        // 挤压动画状态:选中下标 + 动画起点
        let mut sel = 0usize;
        let mut old_sel = 0usize;
        let mut squeeze_start = std::time::Instant::now();
        let mut prev_keys: [bool; 4] = [false; 4]; // Left Up Right Down
        // 帧计时(gdi/降采样/上传/提交/呈现)
        let mut timings: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
        let deadline = start + std::time::Duration::from_secs(12); // 计时演示 12s 即出统计
        println!(
            "候选窗已呈现 {}×{}（透明={}）：水平版 + 垂直版 · 滑入 + 高亮滑动 + 翻页演示",
            extent.width, extent.height, transparent
        );
        loop {
            if std::time::Instant::now() >= deadline {
                break;
            }
            // 处理窗口消息（Esc 已改为触发淡出；WM_DESTROY 直接退出）
            {
                use windows::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, PeekMessageW, TranslateMessage, PM_REMOVE,
                };
                let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == 0x0012 {
                        // WM_QUIT
                        return Ok(());
                    }
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&mut msg);
                }
            }

            let now = std::time::Instant::now();
            let dt = now.duration_since(last).as_secs_f32().min(0.05);
            last = now;
            let t = start.elapsed().as_secs_f32();

            // 淡入
            let fin = (t / FADE_IN).min(1.0);
            let fi = 1.0 - (1.0 - fin).powi(3);
            // 淡出（Esc 后）
            if crate::CLOSING.load(Ordering::Relaxed) && closing_start.is_none() {
                closing_start = Some(now);
            }
            let fade = if let Some(cs) = closing_start {
                let e = cs.elapsed().as_secs_f32();
                let fo = (e / FADE_OUT).min(1.0);
                let f = 1.0 - (1.0 - fo).powi(3); // 0→1
                fi * (1.0 - f)
            } else {
                fi
            };
            if closing_start
                .map_or(false, |cs| cs.elapsed().as_secs_f32() >= FADE_OUT)
            {
                break;
            }

            // 出现滑入（ease-out cubic）
            let y = (BASE_Y as f32 + SLIDE_DIST * (1.0 - fi)) as i32;
            let _ = SetWindowPos(hwnd, None, BASE_X, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);

            // ── 绘制 DIB(超采样):三壳挤压式 ──
            let gdi_start = std::time::Instant::now();
            // 挤压插值:0..1(VSCode ease)
            let sq_t = (squeeze_start.elapsed().as_secs_f32() / SQUEEZE_MS).clamp(0.0, 1.0);
            let pad_s = vscode_ease(sq_t);
            // 箭头键(边沿检测):←↑ = 前一个,→↓ = 后一个
            unsafe {
                use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
                let keys = [
                    GetAsyncKeyState(0x25i32) as u16 & 0x8000 != 0, // VK_LEFT
                    GetAsyncKeyState(0x26i32) as u16 & 0x8000 != 0, // VK_UP
                    GetAsyncKeyState(0x27i32) as u16 & 0x8000 != 0, // VK_RIGHT
                    GetAsyncKeyState(0x28i32) as u16 & 0x8000 != 0, // VK_DOWN
                ];
                for (i, pressed) in keys.iter().enumerate() {
                    if *pressed && !prev_keys[i] {
                        // 边沿:触发挤压动画(换选中)
                        old_sel = sel;
                        sel = (sel + if i >= 2 { 1 } else { 4 }) % 5;
                        squeeze_start = std::time::Instant::now();
                    }
                    prev_keys[i] = *pressed;
                }
            }
            let rects = layout_state(pad_s, sel, old_sel);
            let rise = start.elapsed().as_secs_f32();
            // 三壳绘制(透明区域=壳外,靠 cov_ss 预乘)
            draw_shell(0, rects[0].0, rects[0].1, rects[0].2 - rects[0].0, rects[0].3 - rects[0].1, pad_s, sel, old_sel, rise);
            draw_shell(1, rects[1].0, rects[1].1, rects[1].2 - rects[1].0, rects[1].3 - rects[1].1, pad_s, sel, old_sel, rise);
            draw_shell(2, rects[2].0, rects[2].1, rects[2].2 - rects[2].0, rects[2].3 - rects[2].1, pad_s, sel, old_sel, rise);
            let gdi_ms = gdi_start.elapsed().as_secs_f32() * 1000.0;

            // 拷贝超采样位图 → 降采样源
            let ds_start = std::time::Instant::now();
            let mut dib_pixels = vec![0u8; (dib_w * dib_h * 4) as usize];
            std::ptr::copy_nonoverlapping(
                bits as *const u8,
                dib_pixels.as_mut_ptr(),
                dib_pixels.len(),
            );

            // 降采样 + 预乘 alpha（透明模式下圆角外/缝隙 alpha=0 透出桌面）
            let ss = SS as usize;
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let mut ar = 0u32;
                    let mut ag = 0u32;
                    let mut ab = 0u32;
                    let mut cov = 0.0f32;
                    for sy in 0..ss {
                        for sx in 0..ss {
                            let dx = x * ss + sx;
                            let dy = y * ss + sy;
                            let o = (dy * dib_w as usize + dx) * 4;
                            ab += dib_pixels[o] as u32;
                            ag += dib_pixels[o + 1] as u32;
                            ar += dib_pixels[o + 2] as u32;
                            cov += cov_ss[dy * dib_w as usize + dx];
                        }
                    }
                    let o = (y * width as usize + x) * 4;
                    let a = if transparent { cov / n_ss as f32 * fade } else { 1.0 };
                    pixels[o] = (ab as f32 / n_ss as f32 * a) as u8;
                    pixels[o + 1] = (ag as f32 / n_ss as f32 * a) as u8;
                    pixels[o + 2] = (ar as f32 / n_ss as f32 * a) as u8;
                    pixels[o + 3] = (255.0 * a) as u8;
                }
            }

            let ds_ms = ds_start.elapsed().as_secs_f32() * 1000.0;
            // 上传到 staging buffer（保持 mapped）
            let up_start = std::time::Instant::now();
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), mapped as *mut u8, pixels.len());
            let upload_ms = up_start.elapsed().as_secs_f32() * 1000.0;
            let submit_start = std::time::Instant::now();

            // 等待上一帧完成，再提交本帧
            let _ = device.wait_for_fences(&[fence], true, u64::MAX);
            let _ = device.reset_fences(&[fence]);
            let (image_index, _) = swapchain_fn.acquire_next_image(
                swapchain,
                u64::MAX,
                sem_available,
                vk::Fence::null(),
            )?;
            let _ = device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty());
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let to_dst = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(image)
                .subresource_range(ranges)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_dst],
            );
            device.cmd_copy_buffer_to_image(
                cmd,
                buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::BufferImageCopy::default()
                    .image_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(0)
                            .base_array_layer(0)
                            .layer_count(1),
                    )
                    .image_extent(vk::Extent3D {
                        width: width as u32,
                        height: height as u32,
                        depth: 1,
                    })],
            );
            let to_src = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .image(image)
                .subresource_range(ranges)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
            let swap_to_dst = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .image(images[image_index as usize])
                .subresource_range(ranges)
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_src, swap_to_dst],
            );
            device.cmd_copy_image(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                images[image_index as usize],
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[vk::ImageCopy::default()
                    .src_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(0)
                            .base_array_layer(0)
                            .layer_count(1),
                    )
                    .dst_subresource(
                        vk::ImageSubresourceLayers::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .mip_level(0)
                            .base_array_layer(0)
                            .layer_count(1),
                    )
                    .extent(vk::Extent3D {
                        width: extent.width,
                        height: extent.height,
                        depth: 1,
                    })],
            );
            let to_present = vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                .image(images[image_index as usize])
                .subresource_range(ranges)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE);
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_present],
            );
            device.end_command_buffer(cmd)?;
            device.queue_submit(
                queue,
                &[vk::SubmitInfo::default()
                    .wait_semaphores(&[sem_available])
                    .wait_dst_stage_mask(&[vk::PipelineStageFlags::TRANSFER])
                    .command_buffers(&[cmd])
                    .signal_semaphores(&[sem_rendered])],
                fence,
            )?;
            let submit_ms = submit_start.elapsed().as_secs_f32() * 1000.0;
            let _ = swapchain_fn.queue_present(
                queue,
                &vk::PresentInfoKHR::default()
                    .wait_semaphores(&[sem_rendered])
                    .swapchains(&[swapchain])
                    .image_indices(&[image_index]),
            );
            let present_ms = submit_start.elapsed().as_secs_f32() * 1000.0 - submit_ms;
            timings.push((gdi_ms, ds_ms, upload_ms, submit_ms, present_ms));

            std::thread::sleep(std::time::Duration::from_millis(16));
        }

        // ── 帧计时统计(与 D2D theme.log 对比用)──
        if !timings.is_empty() {
            let mut stat = |idx: usize, name: &str| {
                let mut v: Vec<f32> = timings.iter().map(|t| *t).map(|t| [t.0, t.1, t.2, t.3, t.4][idx]).collect();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let avg = v.iter().sum::<f32>() / v.len() as f32;
                let p50 = v[v.len() / 2];
                let p95 = v[(v.len() as f32 * 0.95) as usize % v.len()];
                println!("  {name}: avg {avg:.2}ms  p50 {p50:.2}ms  p95 {p95:.2}ms");
            };
            println!("=== Vulkan 帧计时({} 帧)===", timings.len());
            stat(0, "GDI绘制 ");
            stat(1, "降采样 ");
            stat(2, "上传   ");
            stat(3, "提交   ");
            stat(4, "呈现   ");
            let total: Vec<f32> = timings.iter().map(|t| t.0 + t.1 + t.2 + t.3 + t.4).collect();
            let mut sorted = total.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            println!(
                "  总计    : avg {:.2}ms  p50 {:.2}ms  p95 {:.2}ms  (≈{:.0} fps)",
                total.iter().sum::<f32>() / total.len() as f32,
                sorted[sorted.len() / 2],
                sorted[(sorted.len() as f32 * 0.95) as usize % sorted.len()],
                1000.0 / (total.iter().sum::<f32>() / total.len() as f32),
            );
        }

        let _ = SelectObject(dc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        device.unmap_memory(buffer_memory);
        device.destroy_fence(fence, None);
        device.destroy_semaphore(sem_rendered, None);
        device.destroy_semaphore(sem_available, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(buffer, None);
        device.free_memory(buffer_memory, None);
        device.destroy_image(image, None);
        device.free_memory(image_memory, None);
        swapchain_fn.destroy_swapchain(swapchain, None);
        device.destroy_device(None);
        surface_fn.destroy_surface(surface, None);
        instance.destroy_instance(None);
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}
