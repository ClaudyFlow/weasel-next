//! Vulkan 版设置界面 · v2：**固定不透明灰色** + GDI 画布贴图 + 真消息循环。
//!
//! 相对 v1（只画矩形、12 秒后退出）的变化：
//! 1. 界面内容改用 `canvas::Canvas::draw_settings()`——GDI 画出**带文字**的整张
//!    设置界面（7 页导航 + 9 张统计卡 + 段位卡），配色是**固定不透明灰阶**；
//! 2. 贴图路径照 `cand.rs`：画布像素 → staging buffer → VkImage → `vkCmdCopyImage`
//!    贴到交换链（仍然不用 shader）；
//! 3. 交换链 `composite_alpha = OPAQUE`——**不透明**，不做玻璃/亚克力；
//! 4. 跑真消息循环，Esc 或关闭窗口才退出（v1 是 sleep 12 秒自毁）。
//!
//! 布局照 D2D 外壳骨架，坐标按 125% DPI 手算（1250×825 DIP = 1563×1031 像素），
//! 与 Slint / D2D 两版同一套页面。
//!
//! `cargo run --release -- ui`

use std::time::Duration;

use ash::{khr, vk, Entry};

use crate::canvas::Canvas;

const WM_QUIT: u32 = 0x0012;
const WM_KEYDOWN: u32 = 0x0100;

/// 按属性挑一个内存类型；找不到就退回 0（照 `cand.rs` 的宽松策略）。
unsafe fn pick_memory_type(
    props: &vk::PhysicalDeviceMemoryProperties,
    type_bits: u32,
    want: vk::MemoryPropertyFlags,
) -> u32 {
    (0..props.memory_type_count)
        .find(|index| {
            type_bits & (1 << index) != 0
                && props.memory_types[*index as usize]
                    .property_flags
                    .contains(want)
        })
        .unwrap_or((0..props.memory_type_count)
            .find(|index| type_bits & (1 << index) != 0)
            .unwrap_or(0))
}

pub unsafe fn run(entry: &Entry) -> Result<(), Box<dyn std::error::Error>> {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW,
        ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, MSG, SW_SHOW, WNDCLASSW, WS_POPUP,
        WS_VISIBLE,
    };

    unsafe {
        // ── 1. 窗口（1250×825 DIP，125% DPI 下客户区 1563×1031）──
        let module = GetModuleHandleW(None)?;
        let class_name = w!("VkSettingWindow");
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(crate::wnd_proc),
            hInstance: module.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wnd_class);
        let (width, height) = (1250i32, 825i32);
        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("Weasel-Next 设置 (Vulkan)"),
            WS_POPUP | WS_VISIBLE,
            200,
            100,
            width,
            height,
            None,
            None,
            Some(module.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);

        // ── 2. instance + surface ──
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0));
        let instance_extensions = [khr::surface::NAME.as_ptr(), khr::win32_surface::NAME.as_ptr()];
        let instance = entry.create_instance(
            &vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_extension_names(&instance_extensions),
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

        // ── 3. 选设备：独显优先（本机第 0 台是 AMD 核显）──
        let mut best: Option<(vk::PhysicalDevice, u32, u32, i32)> = None;
        for device in instance.enumerate_physical_devices()? {
            let props = instance.get_physical_device_properties(device);
            let rank = match props.device_type {
                vk::PhysicalDeviceType::DISCRETE_GPU => 2,
                vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
                _ => 0,
            };
            let families = instance.get_physical_device_queue_family_properties(device);
            let graphics = families
                .iter()
                .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS));
            let present = families.iter().enumerate().position(|(index, _)| {
                surface_fn
                    .get_physical_device_surface_support(device, index as u32, surface)
                    .unwrap_or(false)
            });
            if let (Some(graphics), Some(present)) = (graphics, present) {
                if best.map_or(true, |(_, _, _, current)| rank > current) {
                    best = Some((device, graphics as u32, present as u32, rank));
                }
            }
        }
        let Some((physical, graphics_family, present_family, _)) = best else {
            return Err("没有可用设备".into());
        };
        let props = instance.get_physical_device_properties(physical);
        println!(
            "渲染设备: {}",
            props
                .device_name_as_c_str()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "?".into())
        );

        let priorities = [1.0f32];
        let mut unique = vec![graphics_family];
        if present_family != graphics_family {
            unique.push(present_family);
        }
        let queue_infos: Vec<_> = unique
            .iter()
            .map(|family| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(*family)
                    .queue_priorities(&priorities)
            })
            .collect();
        let device = instance.create_device(
            physical,
            &vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_infos)
                .enabled_extension_names(&[khr::swapchain::NAME.as_ptr()]),
            None,
        )?;
        let queue = device.get_device_queue(graphics_family, 0);
        let swapchain_fn = khr::swapchain::Device::new(&instance, &device);

        // ── 4. 交换链（必须用真实客户区，否则只呈现左上角一块）──
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
            vk::Extent2D { width: width as u32, height: height as u32 }
        };
        println!("交换链尺寸: {}×{}", extent.width, extent.height);
        let mut info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count((caps.min_image_count + 1).min(3))
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            // 贴图路线：交换链要能当 copy 的目标
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
            // 用户要「固定不透明」：不做玻璃/亚克力，直接 OPAQUE
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(vk::PresentModeKHR::FIFO)
            .clipped(true);
        let indices = [graphics_family, present_family];
        if graphics_family != present_family {
            info = info
                .image_sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(&indices);
        }
        let swapchain = swapchain_fn.create_swapchain(&info, None)?;
        let images = swapchain_fn.get_swapchain_images(swapchain)?;

        // ── 5. 画布（GDI 画出带文字的整张界面，固定不透明灰阶）──
        // 缩放按交换链真实宽度算，保证画布像素尺寸 == 交换链尺寸（不再写死 1.25）
        let scale = extent.width as f32 / 1250.0;
        let mut page: usize = 0;
        let mut canvas = Canvas::draw_settings(scale, page);
        println!("画布尺寸: {}×{} (scale {scale:.2})", canvas.width, canvas.height);

        let ranges = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .base_mip_level(0)
            .level_count(1)
            .base_array_layer(0)
            .layer_count(1);
        let mem_props = instance.get_physical_device_memory_properties(physical);

        // 源 image（放画布像素）
        let image = device.create_image(
            &vk::ImageCreateInfo::default()
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::B8G8R8A8_UNORM)
                .extent(vk::Extent3D {
                    width: canvas.width as u32,
                    height: canvas.height as u32,
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
        let image_req = device.get_image_memory_requirements(image);
        let image_memory = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(image_req.size)
                .memory_type_index(pick_memory_type(
                    &mem_props,
                    image_req.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )),
            None,
        )?;
        device.bind_image_memory(image, image_memory, 0)?;

        // staging buffer（HOST 可见 + 一致，省掉手动 flush）
        let byte_len = canvas.pixels.len() as u64;
        let buffer = device.create_buffer(
            &vk::BufferCreateInfo::default()
                .size(byte_len)
                .usage(vk::BufferUsageFlags::TRANSFER_SRC),
            None,
        )?;
        let buffer_req = device.get_buffer_memory_requirements(buffer);
        let buffer_memory = device.allocate_memory(
            &vk::MemoryAllocateInfo::default()
                .allocation_size(buffer_req.size)
                .memory_type_index(pick_memory_type(
                    &mem_props,
                    buffer_req.memory_type_bits,
                    vk::MemoryPropertyFlags::HOST_VISIBLE
                        | vk::MemoryPropertyFlags::HOST_COHERENT,
                )),
            None,
        )?;
        device.bind_buffer_memory(buffer, buffer_memory, 0)?;
        let mapped = device.map_memory(buffer_memory, 0, byte_len, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(
            canvas.pixels.as_ptr(),
            mapped as *mut u8,
            canvas.pixels.len(),
        );
        // 保持映射：翻页时直接覆盖像素并重新上传（不再 unmap）

        let pool = device.create_command_pool(
            &vk::CommandPoolCreateInfo::default()
                .queue_family_index(graphics_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
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

        // ── 6. 上传：buffer → image，之后 image 常驻 TRANSFER_SRC ──
        upload_canvas(
            &device,
            queue,
            cmd,
            fence,
            buffer,
            image,
            ranges,
            canvas.width as u32,
            canvas.height as u32,
        )?;

        // 画布与交换链尺寸可能不等（DPI 不是 125% 时）：取交集，别越界
        let copy_w = (canvas.width as u32).min(extent.width);
        let copy_h = (canvas.height as u32).min(extent.height);

        // ── 7. 消息循环 + 每帧贴图 ──
        let mut running = true;
        let mut msg = MSG::default();
        while running {
            while PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    running = false;
                }
                // 数字键 1–7 切页（翻页时重画画布并重新上传到 image）
                if msg.message == WM_KEYDOWN
                    && (0x31..=0x37).contains(&(msg.wParam.0 as u8))
                {
                    let np = (msg.wParam.0 as u8 - 0x31) as usize;
                    if np != page {
                        page = np;
                        canvas = Canvas::draw_settings(scale, page);
                        std::ptr::copy_nonoverlapping(
                            canvas.pixels.as_ptr(),
                            mapped as *mut u8,
                            canvas.pixels.len(),
                        );
                        let _ = upload_canvas(
                            &device, queue, cmd, fence, buffer, image, ranges,
                            canvas.width as u32, canvas.height as u32,
                        );
                    }
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !running {
                break;
            }

            let _ = device.wait_for_fences(&[fence], true, u64::MAX);
            let _ = device.reset_fences(&[fence]);
            let Ok((image_index, _)) = swapchain_fn.acquire_next_image(
                swapchain,
                u64::MAX,
                sem_available,
                vk::Fence::null(),
            ) else {
                continue;
            };
            let _ = device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty());
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            // 交换链图：UNDEFINED → TRANSFER_DST
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::UNDEFINED)
                    .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .image(images[image_index as usize])
                    .subresource_range(ranges)
                    .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)],
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
                    .extent(vk::Extent3D { width: copy_w, height: copy_h, depth: 1 })],
            );
            // TRANSFER_DST → PRESENT_SRC
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[vk::ImageMemoryBarrier::default()
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
                    .image(images[image_index as usize])
                    .subresource_range(ranges)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)],
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
            // FIFO 已按 vsync 限帧，这里只让出一点 CPU
            std::thread::sleep(Duration::from_millis(1));
        }

        // ── 8. 清理 ──
        let _ = device.wait_for_fences(&[fence], true, u64::MAX);
        device.destroy_fence(fence, None);
        device.destroy_semaphore(sem_rendered, None);
        device.destroy_semaphore(sem_available, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(buffer, None);
        device.unmap_memory(buffer_memory);
        device.free_memory(buffer_memory, None);
        device.destroy_image(image, None);
        device.free_memory(image_memory, None);
        swapchain_fn.destroy_swapchain(swapchain, None);
        device.destroy_device(None);
        surface_fn.destroy_surface(surface, None);
        instance.destroy_instance(None);
        let _ = DestroyWindow(hwnd);
        println!("已退出");
    }
    Ok(())
}

/// 把已映射的 staging buffer 里的画布像素拷贝进 image，并让 image 停在
/// TRANSFER_SRC_OPTIMAL（供每帧 `cmd_copy_image` 贴到交换链）。翻页时复用。
unsafe fn upload_canvas(
    device: &ash::Device,
    queue: vk::Queue,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    buffer: vk::Buffer,
    image: vk::Image,
    ranges: vk::ImageSubresourceRange,
    w: u32,
    h: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    use vk::AccessFlags as AF;
    use vk::PipelineStageFlags as PS;
    device.wait_for_fences(&[fence], true, u64::MAX)?;
    device.reset_fences(&[fence])?;
    device.begin_command_buffer(
        cmd,
        &vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
    )?;
    device.cmd_pipeline_barrier(
        cmd,
        PS::TOP_OF_PIPE,
        PS::TRANSFER,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(image)
            .subresource_range(ranges)
            .dst_access_mask(AF::TRANSFER_WRITE)],
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
                width: w,
                height: h,
                depth: 1,
            })],
    );
    device.cmd_pipeline_barrier(
        cmd,
        PS::TRANSFER,
        PS::TRANSFER,
        vk::DependencyFlags::empty(),
        &[],
        &[],
        &[vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .image(image)
            .subresource_range(ranges)
            .src_access_mask(AF::TRANSFER_WRITE)
            .dst_access_mask(AF::TRANSFER_READ)],
    );
    device.end_command_buffer(cmd)?;
    device.queue_submit(queue, &[vk::SubmitInfo::default().command_buffers(&[cmd])], fence)?;
    device.wait_for_fences(&[fence], true, u64::MAX)?;
    Ok(())
}
