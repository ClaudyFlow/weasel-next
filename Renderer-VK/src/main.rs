//! Vulkan 渲染器原型(2026-11-01 从 Setting-vk 拆出):
//! 候选窗(cand)/ 背景模糊(blur)/ 后端探测(probe)/ 交换链实验(window/dump/enum)。
//! 设置界面在 ../Setting-vk(两线分离,各自演进)。
//!
//! `cand`   → 候选窗原型(VkImage + GDI 内容)
//! `probe`  → Vulkan/DXGI/D3D11 后端探测
//! `window` → 交换链 200 帧计时
//! (默认)  → 只枚举设备

mod blur;
mod cand;
mod probe;

use std::time::Instant;

use ash::{khr, vk, Entry};

const WM_DESTROY: u32 = 0x0002;
const WM_KEYDOWN: u32 = 0x0100;
const WM_NCHITTEST: u32 = 0x0084;
const HTCAPTION: isize = 2;
const VK_ESCAPE: usize = 0x1B;

pub(crate) unsafe extern "system" fn wnd_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe {
        match msg {
            // 窗口被销毁 / 按 Esc：让消息循环能退出（PostQuitMessage 投 WM_QUIT）
            WM_DESTROY => {
                windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                windows::Win32::Foundation::LRESULT(0)
            }
            // 整个窗口可拖动：命中测试返回"标题栏"，系统接管鼠标移动（用户要看半透明效果）
            WM_NCHITTEST => windows::Win32::Foundation::LRESULT(HTCAPTION),
            WM_KEYDOWN if wp.0 == VK_ESCAPE => {
                windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                windows::Win32::Foundation::LRESULT(0)
            }
            _ => windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    unsafe {
        let entry = Entry::load()?;
        match mode.as_str() {
            "window" => run_window(&entry)?,
            "probe" => probe::run(&entry)?,
            "cand" => cand::run(&entry)?,
            _ => run_enum(&entry)?,
        }
    }
    Ok(())
}

unsafe fn run_enum(entry: &Entry) -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0));
        let instance =
            entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None)?;
        for (index, device) in instance.enumerate_physical_devices()?.iter().enumerate() {
            let props = instance.get_physical_device_properties(*device);
            let name = props
                .device_name_as_c_str()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "?".into());
            println!("[{index}] {name}  {:?}", props.device_type);
        }
        instance.destroy_instance(None);
    }
    Ok(())
}

unsafe fn run_window(entry: &Entry) -> Result<(), Box<dyn std::error::Error>> {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, RegisterClassW, ShowWindow, CS_HREDRAW, CS_VREDRAW,
        SW_SHOW, WNDCLASSW, WS_POPUP, WS_VISIBLE,
    };

    unsafe {
        // 1. 窗口
        let module = GetModuleHandleW(None)?;
        let class_name = w!("VkProbeWindow");
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: module.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassW(&wnd_class);
        let (width, height) = (520i32, 160i32);
        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("vk probe"),
            WS_POPUP | WS_VISIBLE,
            600,
            400,
            width,
            height,
            None,
            None,
            Some(module.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);

        // 2. instance + surface
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0));
        let instance_extensions = [khr::surface::NAME.as_ptr(), khr::win32_surface::NAME.as_ptr()];
        let instance = entry.create_instance(
            &vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_extension_names(&instance_extensions),
            None,
        )?;
        let win32_fn = khr::win32_surface::Instance::new(entry, &instance);
        let surface_info = vk::Win32SurfaceCreateInfoKHR::default()
            .hinstance(module.0 as isize)
            .hwnd(hwnd.0 as isize);
        let surface = win32_fn.create_win32_surface(&surface_info, None)?;
        let surface_fn = khr::surface::Instance::new(entry, &instance);

        // 3. 选设备（graphics + present）
        let mut chosen = None;
        for device in instance.enumerate_physical_devices()? {
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
                chosen = Some((device, graphics as u32, present as u32));
                break;
            }
        }
        let Some((physical, graphics_family, present_family)) = chosen else {
            return Err("没有同时支持 graphics 与 present 的设备".into());
        };
        let props = instance.get_physical_device_properties(physical);
        println!(
            "设备: {}",
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
        let device_extensions = [khr::swapchain::NAME.as_ptr()];
        let device = instance.create_device(
            physical,
            &vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_infos)
                .enabled_extension_names(&device_extensions),
            None,
        )?;
        let queue = device.get_device_queue(graphics_family, 0);
        let swapchain_fn = khr::swapchain::Device::new(&instance, &device);

        // 4. 交换链
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
        let mut info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count((caps.min_image_count + 1).min(3))
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(caps.current_transform)
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

        let attachment = vk::AttachmentDescription::default()
            .format(format.format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
        let color_ref = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpass = vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&color_ref);
        let render_pass = device.create_render_pass(
            &vk::RenderPassCreateInfo::default()
                .attachments(&[attachment])
                .subpasses(&[subpass]),
            None,
        )?;

        let mut views = Vec::new();
        let mut framebuffers = Vec::new();
        for image in &images {
            let view = device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(*image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format.format)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::COLOR)
                            .base_mip_level(0)
                            .level_count(1)
                            .base_array_layer(0)
                            .layer_count(1),
                    ),
                None,
            )?;
            views.push(view);
            framebuffers.push(device.create_framebuffer(
                &vk::FramebufferCreateInfo::default()
                    .render_pass(render_pass)
                    .attachments(&[view])
                    .width(extent.width)
                    .height(extent.height)
                    .layers(1),
                None,
            )?);
        }

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

        // 5. 200 帧
        let frames = 200usize;
        let mut timings = Vec::with_capacity(frames);
        for frame in 0..frames {
            let start = Instant::now();
            let _ = device.wait_for_fences(&[fence], true, u64::MAX);
            let _ = device.reset_fences(&[fence]);
            let Ok((image_index, _)) = swapchain_fn.acquire_next_image(
                swapchain,
                u64::MAX,
                sem_available,
                vk::Fence::null(),
            ) else {
                break;
            };
            let _ = device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty());
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let t = frame as f32 / frames as f32;
            let clear = [vk::ClearValue {
                color: vk::ClearColorValue {
                    float32: [0.12 + t * 0.2, 0.14, 0.30 - t * 0.1, 1.0],
                },
            }];
            device.cmd_begin_render_pass(
                cmd,
                &vk::RenderPassBeginInfo::default()
                    .render_pass(render_pass)
                    .framebuffer(framebuffers[image_index as usize])
                    .render_area(vk::Rect2D {
                        offset: Default::default(),
                        extent,
                    })
                    .clear_values(&clear),
                vk::SubpassContents::INLINE,
            );
            device.cmd_end_render_pass(cmd);
            device.end_command_buffer(cmd)?;
            let wait = [sem_available];
            let signal = [sem_rendered];
            let stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            device.queue_submit(
                queue,
                &[vk::SubmitInfo::default()
                    .wait_semaphores(&wait)
                    .wait_dst_stage_mask(&stages)
                    .command_buffers(&[cmd])
                    .signal_semaphores(&signal)],
                fence,
            )?;
            let _ = swapchain_fn.queue_present(
                queue,
                &vk::PresentInfoKHR::default()
                    .wait_semaphores(&signal)
                    .swapchains(&[swapchain])
                    .image_indices(&[image_index]),
            );
            let _ = device.wait_for_fences(&[fence], true, u64::MAX);
            timings.push(start.elapsed().as_secs_f64() * 1000.0);
        }

        timings.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let avg: f64 = timings.iter().sum::<f64>() / timings.len().max(1) as f64;
        let p95 = timings.get((timings.len() as f64 * 0.95) as usize).copied().unwrap_or(avg);
        println!(
            "{} 帧 · 平均 {avg:.2} ms · P95 {p95:.2} ms · 最小 {:.2} · 最大 {:.2}",
            timings.len(),
            timings.first().copied().unwrap_or(0.0),
            timings.last().copied().unwrap_or(0.0)
        );

        device.destroy_fence(fence, None);
        device.destroy_semaphore(sem_rendered, None);
        device.destroy_semaphore(sem_available, None);
        device.destroy_command_pool(pool, None);
        for framebuffer in &framebuffers {
            device.destroy_framebuffer(*framebuffer, None);
        }
        for view in &views {
            device.destroy_image_view(*view, None);
        }
        device.destroy_render_pass(render_pass, None);
        swapchain_fn.destroy_swapchain(swapchain, None);
        device.destroy_device(None);
        surface_fn.destroy_surface(surface, None);
        instance.destroy_instance(None);
        let _ = DestroyWindow(hwnd);
        println!("OK");
    }
    Ok(())
}
