//! 「自己检测渲染库」：把机器上能用的后端都探一遍，最后给出推荐。
//!
//! 探的东西：
//!   * Vulkan —— `vulkan-1.dll` 能否加载、几台设备、有没有独显、有没有 D3D 互操作扩展
//!   * DXGI   —— 适配器列表（名字 / 显存 / 是否软件渲染）
//!   * D3D11  —— `D3D11CreateDevice` 能不能建**硬件**设备、拿到 feature level

use ash::{vk, Entry};

pub unsafe fn run(entry: &Entry) -> Result<(), Box<dyn std::error::Error>> {
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_DESC1, DXGI_ERROR_NOT_FOUND,
    };

    unsafe {
        // ---------- Vulkan ----------
        println!("=== Vulkan ===");
        let app = vk::ApplicationInfo::default().api_version(vk::make_api_version(0, 1, 3, 0));
        let instance =
            entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None)?;
        let devices = instance.enumerate_physical_devices()?;
        let mut vulkan_discrete = false;
        for (index, device) in devices.iter().enumerate() {
            let props = instance.get_physical_device_properties(*device);
            let name = props
                .device_name_as_c_str()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "?".into());
            let discrete = props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU;
            vulkan_discrete |= discrete;
            let extensions = instance.enumerate_device_extension_properties(*device)?;
            let has_interop = extensions.iter().any(|extension| {
                extension
                    .extension_name_as_c_str()
                    .map(|name| name.to_string_lossy() == "VK_KHR_external_memory_win32")
                    .unwrap_or(false)
            });
            println!(
                "  [{index}] {name}  {:?}  互操作={}",
                props.device_type,
                if has_interop { "有" } else { "无" }
            );
        }
        instance.destroy_instance(None);

        // ---------- DXGI ----------
        println!("\n=== DXGI 适配器 ===");
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut index = 0u32;
        let mut dxgi_discrete = false;
        loop {
            match factory.EnumAdapters1(index) {
                Ok(adapter) => {
                    let desc: DXGI_ADAPTER_DESC1 = adapter.GetDesc1()?;
                    let end = desc.Description.iter().position(|c| *c == 0).unwrap_or(128);
                    let name = String::from_utf16_lossy(&desc.Description[..end]);
                    let dedicated = desc.DedicatedVideoMemory as u64 / (1024 * 1024);
                    let software = desc.Flags & 2 != 0; // DXGI_ADAPTER_FLAG_SOFTWARE
                    dxgi_discrete |= !software && dedicated > 512;
                    println!(
                        "  [{index}] {name}  显存 {dedicated} MB  vendor=0x{:04X}  software={software}",
                        desc.VendorId
                    );
                    index += 1;
                }
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(error) => return Err(error.into()),
            }
        }

        // ---------- D3D11 ----------
        println!("\n=== D3D11 硬件设备 ===");
        let mut device: Option<ID3D11Device> = None;
        let mut feature = D3D_FEATURE_LEVEL_11_0;
        let levels = [
            windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
        ];
        let result = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature),
            None,
        );
        match result {
            Ok(()) => println!("  可创建：feature level {feature:?}"),
            Err(error) => println!("  不可创建：{error}"),
        }

        // ---------- 推荐 ----------
        println!("\n=== 推荐后端 ===");
        let recommendation = if vulkan_discrete && dxgi_discrete {
            "vulkan（独显 + 有 D3D 互操作扩展）；D3D11 作兜底"
        } else if device.is_some() {
            "d3d11（Vulkan 只有核显或缺互操作扩展）"
        } else {
            "d3d11-warp（没有可用硬件设备）"
        };
        println!("  {recommendation}");
    }
    Ok(())
}
