//! Vulkan 版候选窗 · 原型：小窗口 + 圆角面板 + 候选文字。
//!
//! 管线（刻意不用 shader）：GDI 把候选窗画进 DIB → 上传 VkImage →
//! `vkCmdBlitImage` 贴到交换链。文字用 GDI 的中文渲染，效果和官方 D2D 版接近。
//!
//! 透明这一步还没接：现在是 `OPAQUE` 交换链，所以面板外是深色底。
//! 下一步换 `PRE_MULTIPLIED` + 圆角 SDF（或继续 CPU 圆角遮罩）。

use ash::{khr, vk, Entry};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject,
    FillRect, SelectObject, SetBkMode, SetTextColor, TextOutW, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, RegisterClassW, ShowWindow, CS_HREDRAW, CS_VREDRAW, SW_SHOW,
    WNDCLASSW, WS_POPUP, WS_VISIBLE,
};

pub unsafe fn run(entry: &Entry) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let scale = 1.25f32;
        let (font_h, pad) = (20.0f32, 10.0f32);
        let row_h = font_h * 1.5;

        // 用户 25:0x：**窗口宽度按实际内容算**，不再写死 760。
        // 先用一个临时 DC 量出每段文字的像素宽，再决定窗口/画布宽度。
        let items = [
            ("1. 今天天气", true),
            ("2. 今天", false),
            ("3. 金田", false),
            ("4. 锦天", false),
            ("5. 尽甜", false),
        ];
        let preedit = "jin'tian'tian'qi";
        let scratch = CreateCompatibleDC(None);
        let mut measure = |content: &str, bold: bool| -> f32 {
            let font = CreateFontW(
                (font_h * scale) as i32, 0, 0, 0,
                if bold { 700 } else { 400 },
                0, 0, 0,
                windows::Win32::Graphics::Gdi::FONT_CHARSET(1),
                windows::Win32::Graphics::Gdi::FONT_OUTPUT_PRECISION(0),
                windows::Win32::Graphics::Gdi::FONT_CLIP_PRECISION(0),
                windows::Win32::Graphics::Gdi::FONT_QUALITY(5),
                0,
                w!("Microsoft YaHei UI"),
            );
            let old = SelectObject(scratch, font.into());
            let wide: Vec<u16> = content.encode_utf16().collect();
            // 用 **墨迹宽度**：GetTextExtentExPointW 会把每个字符的右边界填进 partial，
            // 末值就是最后一点墨迹的位置（不含右侧字距）。步进宽度比它宽几个像素，
            // 之前"框到文字 10px 看着像 20"就是这个误差。
            let mut partial = vec![0i32; wide.len()];
            let mut fit = 0i32;
            let mut size = windows::Win32::Foundation::SIZE::default();
            let _ = windows::Win32::Graphics::Gdi::GetTextExtentExPointW(
                scratch,
                windows::core::PCWSTR(wide.as_ptr()),
                wide.len() as i32,
                i32::MAX,
                Some(&mut fit),
                Some(partial.as_mut_ptr()),
                &mut size,
            );
            let _ = SelectObject(scratch, old);
            let _ = DeleteObject(font.into());
            partial.last().copied().unwrap_or(size.cx) as f32
        };
        let gap_items = font_h; // 候选之间 = 一个全角空格（用户 00:3x）
        let candidates_w: f32 = items
            .iter()
            .map(|(label, bold)| measure(label, *bold))
            .sum::<f32>()
            + gap_items * (items.len() as f32 - 1.0);
        let arrows_w = 36.0f32;
        let content_w = (font_h + font_h / 2.0) + candidates_w.max(measure(preedit, false)) + arrows_w + (font_h + font_h / 2.0);
        // 每条候选的宽度在这里一次算完（scratch 还活着）；绘制阶段直接用。
        let widths: Vec<f32> = items.iter().map(|(label, bold)| measure(label, *bold)).collect();
        let _ = DeleteDC(scratch);

        let width = (content_w * scale) as i32;
        let height = ((pad + font_h + pad + row_h + pad) * scale) as i32;

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
            // 置顶(0x8) + 工具窗(0x80)：不进任务栏、不会被别的窗口压住
            windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(0x0000_0008 | 0x0000_0080), // 去掉 NOREDIRECTIONBITMAP，让 PRE_MULTIPLIED alpha 生效
            class_name,
            w!("候选窗 (Vulkan)"),
            WS_POPUP | WS_VISIBLE,
            300,
            260,
            width,
            height,
            None,
            None,
            Some(module.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);

        // ---- GDI 画候选窗 ----
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
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
        // 高斯模糊背景：抓窗口背后的画面，降采样 + 三趟箱式模糊（≈高斯）。
        let blur_bg = crate::blur::capture_blurred(300, 260, width, height);
        let mut fill = |x: f32, y: f32, w: f32, h: f32, color: u32| {
            let brush = CreateSolidBrush(COLORREF(color));
            let rect = RECT {
                left: (x * scale) as i32,
                top: (y * scale) as i32,
                right: ((x + w) * scale) as i32,
                bottom: ((y + h) * scale) as i32,
            };
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(brush.into());
        };
        // 面板底 + 顶部高光带（金属那套的简化版）
        // 75% 灰（#BFBFBF），不透明：用户 11:1x 要求"做成 75% 灰、无透明"
        fill(0.0, 0.0, 760.0, 80.0, 0x00333333); // #FFFFFF × 20% = #333333
        let mut text = |content: &str, x: f32, y: f32, size: f32, bold: bool, color: u32| {
            let font = CreateFontW(
                (size * scale) as i32,
                0,
                0,
                0,
                if bold { 700 } else { 400 },
                0,
                0,
                0,
                windows::Win32::Graphics::Gdi::FONT_CHARSET(1),
                windows::Win32::Graphics::Gdi::FONT_OUTPUT_PRECISION(0),
                windows::Win32::Graphics::Gdi::FONT_CLIP_PRECISION(0),
                windows::Win32::Graphics::Gdi::FONT_QUALITY(5),
                0,
                w!("Microsoft YaHei UI"),
            );
            let old_font = SelectObject(dc, font.into());
            let _ = SetTextColor(dc, COLORREF(color));
            let wide: Vec<u16> = content.encode_utf16().collect();
            let _ = TextOutW(dc, (x * scale) as i32, (y * scale) as i32, &wide);
            let _ = SelectObject(dc, old_font);
            let _ = DeleteObject(font.into());
        };
        text("jin'tian'tian'qi", 24.0, pad, font_h, false, 0x00D6DEE8);
        text("‹    ›", 660.0, pad, font_h, false, 0x0088A0C8);
        let row_y = pad + font_h + pad;
        let text_y = row_y + (row_h - font_h) / 2.0;
        let mut cursor = font_h + font_h / 2.0; // 左侧：全角 20 + 半角 10
        let mut first_hit = true;
        for (index, (label, bold)) in items.iter().enumerate() {
            let label_w = widths[index];
            if first_hit {
                // 高亮框：GDI 的 RoundRect 没有抗锯齿（用户 00:2x 一直说锯齿）
                // ⇒ 直接往 DIB 的像素里按 SDF 覆盖率写，圆角边缘 2×2 超采样。
                let x0 = ((cursor - 10.0) * scale) as i32; // 内侧留白（已扣掉 GDI 步进宽度里的字距）
                let x1 = ((cursor + label_w + 10.0) * scale) as i32;
                let y0 = (row_y * scale) as i32;
                let y1 = ((row_y + row_h) * scale) as i32;
                let radius = 10.0 * scale;
                let (r, g, b) = (0x3Bu32 as f32, 0x4Au32 as f32, 0x63u32 as f32);
                for py in y0.max(0)..y1.min(height) {
                    for px in x0.max(0)..x1.min(width) {
                        let fx = px as f32 + 0.5;
                        let fy = py as f32 + 0.5;
                        let mut coverage = 0.0f32;
                        for sub_y in 0..2 {
                            for sub_x in 0..2 {
                                let sx = fx + (sub_x as f32 - 0.5) * 0.5;
                                let sy = fy + (sub_y as f32 - 0.5) * 0.5;
                                let dx = if sx < x0 as f32 + radius {
                                    (x0 as f32 + radius) - sx
                                } else if sx > x1 as f32 - radius {
                                    sx - (x1 as f32 - radius)
                                } else {
                                    0.0
                                };
                                let dy = if sy < y0 as f32 + radius {
                                    (y0 as f32 + radius) - sy
                                } else if sy > y1 as f32 - radius {
                                    sy - (y1 as f32 - radius)
                                } else {
                                    0.0
                                };
                                let w = if dx > 0.0 && dy > 0.0 {
                                    (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0)
                                } else {
                                    1.0
                                };
                                coverage += w;
                            }
                        }
                        let coverage = coverage / 4.0;
                        if coverage <= 0.0 {
                            continue;
                        }
                        let offset = ((py * width + px) * 4) as usize;
                        let dst = (bits as *mut u8).add(offset);
                        let blend = |old: u8, new: f32| {
                            (old as f32 * (1.0 - coverage) + new * coverage) as u8
                        };
                        *dst = blend(*dst, b);
                        *dst.add(1) = blend(*dst.add(1), g);
                        *dst.add(2) = blend(*dst.add(2), r);
                    }
                }
                first_hit = false;
            }
            text(label, cursor, text_y, font_h, *bold, if *bold { 0x00FFFFFF } else { 0x00E8EDF4 });
            cursor += label_w + 10.0 + font_h; // 框内 10 + 框外 20
        }

        let mut pixels = vec![0u8; (width * height * 4) as usize];
        std::ptr::copy_nonoverlapping(bits as *const u8, pixels.as_mut_ptr(), pixels.len());
        // 外侧圆角（用户 23:5x：大候选框也要圆角）+ 预乘 alpha：
        // GDI 写出来的 alpha 恒为 0，必须自己按"圆角矩形覆盖率"填，并做预乘。
        let radius = 20.0f32 * scale; // 圆角 20（用户最终定）
        let (fw, fh) = (width as f32, height as f32);
        for y in 0..height as usize {
            for x in 0..width as usize {
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let dx = if fx < radius { radius - fx } else if fx > fw - radius { fx - (fw - radius) } else { 0.0 };
                let dy = if fy < radius { radius - fy } else if fy > fh - radius { fy - (fh - radius) } else { 0.0 };
                // 圆角边缘做 2×2 超采样（抗锯齿）：四个子样本的覆盖率取平均
                let coverage = if dx > 0.0 && dy > 0.0 {
                    let mut sum = 0.0f32;
                    for sub_y in 0..2 {
                        for sub_x in 0..2 {
                            let sx = fx + (sub_x as f32 - 0.5) * 0.5;
                            let sy = fy + (sub_y as f32 - 0.5) * 0.5;
                            let sdx = if sx < radius { radius - sx } else if sx > fw - radius { sx - (fw - radius) } else { 0.0 };
                            let sdy = if sy < radius { radius - sy } else if sy > fh - radius { sy - (fh - radius) } else { 0.0 };
                            let value = if sdx > 0.0 && sdy > 0.0 {
                                (radius - (sdx * sdx + sdy * sdy).sqrt() + 0.5).clamp(0.0, 1.0)
                            } else {
                                1.0
                            };
                            sum += value;
                        }
                    }
                    sum / 4.0
                } else {
                    1.0
                };
                let index = (y * width as usize + x) * 4;
                let alpha = (coverage * 255.0 * 0.2) as u32; // 对齐 D2D 玻璃档：GLASS_ALPHA=0x33(51)≈20%
                for channel in 0..3 {
                    pixels[index + channel] = ((pixels[index + channel] as u32 * alpha) / 255) as u8;
                }
                pixels[index + 3] = alpha as u8;
            }
        }
        let _ = SelectObject(dc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);

        // ---- Vulkan ----
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
                width: width as u32,
                height: height as u32,
            }
        };
        let swapchain = swapchain_fn.create_swapchain(
            &vk::SwapchainCreateInfoKHR::default()
                .surface(surface)
                .min_image_count((caps.min_image_count + 1).min(3))
                .image_format(format.format)
                .image_color_space(format.color_space)
                .image_extent(extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(caps.current_transform)
                .composite_alpha(if caps.supported_composite_alpha.contains(vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED) { vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED } else { vk::CompositeAlphaFlagsKHR::OPAQUE })
                .present_mode(vk::PresentModeKHR::FIFO)
                .clipped(true),
            None,
        )?;
        let images = swapchain_fn.get_swapchain_images(swapchain)?;

        // 上传用的 image / buffer
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

        let byte_len = pixels.len() as u64;
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
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), mapped as *mut u8, pixels.len());
        device.unmap_memory(buffer_memory);

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
        // 尺寸一致时用 copy_image 比 blit 稳（blit 对格式/缩放有额外要求）
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
        let _ = swapchain_fn.queue_present(
            queue,
            &vk::PresentInfoKHR::default()
                .wait_semaphores(&[sem_rendered])
                .swapchains(&[swapchain])
                .image_indices(&[image_index]),
        );
        let _ = device.wait_for_fences(&[fence], true, u64::MAX);
        println!("候选窗已呈现 {}×{}，保持 12 秒", extent.width, extent.height);
                // 消息循环 180 秒：拖动、Esc、重画都靠它，硬 sleep 会让窗口卡死
        {
            use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PeekMessageW, TranslateMessage, DispatchMessageW, PM_REMOVE};
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
            while std::time::Instant::now() < deadline {
                let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == 0x0012 { // WM_QUIT
                        break;
                    }
                    let _ = TranslateMessage(&msg);
                    let _ = DispatchMessageW(&mut msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }

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
