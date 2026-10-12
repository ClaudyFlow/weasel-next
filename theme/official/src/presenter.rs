//! 照官方 WeaselUI 的实现搬过来的呈现层：D3D11 + DXGI 交换链 + DirectComposition + D2D 设备上下文。
//!
//! 对应官方源码 `WeaselUI\d2d.cpp` 的 `D2D::InitDirect2D()`（第 155-267 行）与 `D2D::OnResize()`，
//! 以及 `WeaselPanel.cpp` 里把窗口建成 `WS_EX_NOREDIRECTIONBITMAP` 的那一处。
//!
//! 为什么这么做：窗口没有重定向表面时，内容由 DirectComposition 的视觉直接交给 DWM 合成，
//! D2D 设备上下文的预乘 alpha 会**逐像素**生效 —— 面板可以半透明而文字保持不透明，
//! 这是官方候选窗的做法，也是我们"和官方一模一样"的基础。
//!
//! 本模块只负责平台层的建立与每帧提交；画什么由 `mod.rs` 决定。按用户要求这里**不做磨砂玻璃**
//! （不调用 DWM 的 blur 属性、不做背景模糊），与官方一致：面板用配置里的颜色填充，
//! 阴影用 D2D 的 CLSID_D2D1GaussianBlur 画在几何体上。
//!
//! 编号与官方一一对应，便于逐行核对：
//!   ① 创建 D3D11 设备（BGRA 支持）
//!   ② 取 IDXGIDevice，建 IDXGIFactory2
//!   ③ CreateSwapChainForComposition（B8G8R8A8_UNORM / PREMULTIPLIED / FLIP_SEQUENTIAL / BufferCount 2）
//!   ④ 取后缓冲为 IDXGISurface2
//!   ⑤ D2D1CreateDevice 建 ID2D1Device1，CreateDeviceContext 建设备上下文
//!   ⑥ CreateBitmapFromDxgiSurface（TARGET | CANNOT_DRAW）并 SetTarget
//!   ⑦ DCompositionCreateDevice → CreateTargetForHwnd(topmost) → CreateVisual → SetContent → SetRoot → Commit
//!   ⑧ 建一支实色画刷
//!
//! 注意：所有 COM 资源都必须由创建它们的 UI 线程释放，因此本结构体不实现 `Send`/`Sync`。

#![allow(dead_code)]

use crate::d2d_bindings::*;
use windows_core::{Interface, HSTRING};

/// 一次呈现所需的全部平台资源。字段顺序即释放顺序的反序，保持与创建顺序一致便于排查。
pub(crate) struct Presenter {
    device: ID3D11Device,
    dxgi_device: IDXGIDevice,
    factory: IDXGIFactory2,
    swap_chain: IDXGISwapChain1,
    surface: IDXGISurface2,
    d2d_device: ID2D1Device1,
    context: ID2D1DeviceContext,
    target_bitmap: ID2D1Bitmap1,
    dcomp: IDCompositionDevice,
    dcomp_target: IDCompositionTarget,
    visual: IDCompositionVisual,
    width: u32,
    height: u32,
}

impl Presenter {
    /// ①-⑧：按官方顺序建立全部资源。窗口必须已经创建并且有非零客户区。
    pub(crate) unsafe fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("客户区尺寸为 0，官方在这里直接返回".into());
        }

        // ① D3D11 设备：必须带 BGRA 支持，否则 D2D 无法与之互操作。
        let mut device: Option<ID3D11Device> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;
        let mut d3d_context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_10_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut d3d_context),
        )
        .map_err(|error| format!("D3D11CreateDevice 失败: {error}"))?;
        let device = device.ok_or_else(|| "D3D11CreateDevice 未返回设备".to_string())?;

        // ② 取 DXGI 设备与工厂（官方用共享的 DeviceResources，我们是每个窗口自建一份）。
        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|error| format!("ID3D11Device → IDXGIDevice 失败: {error}"))?;
        let adapter = dxgi_device
            .GetAdapter()
            .map_err(|error| format!("GetAdapter 失败: {error}"))?;
        let factory: IDXGIFactory2 = adapter
            .GetParent()
            .map_err(|error| format!("GetParent(IDXGIFactory2) 失败: {error}"))?;

        // ③ 组合用交换链：官方参数原样照抄。
        let description = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap_chain = factory
            .CreateSwapChainForComposition(&dxgi_device, &description, None)
            .map_err(|error| format!("CreateSwapChainForComposition 失败: {error}"))?;

        // ④ 后缓冲 → IDXGISurface2。
        let surface: IDXGISurface2 = swap_chain
            .GetBuffer(0)
            .map_err(|error| format!("GetBuffer(0) 失败: {error}"))?;

        // ⑤ D2D 设备与设备上下文。
        let d2d_factory: ID2D1Factory1 = unsafe {
            let mut factory: Option<ID2D1Factory1> = None;
            D2D1CreateFactory(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                &ID2D1Factory1::IID,
                None,
                (&mut factory as *mut Option<ID2D1Factory1>).cast(),
            )
            .ok()
            .map_err(|error| format!("D2D1CreateFactory 失败: {error}"))?;
            factory.ok_or_else(|| "D2D1CreateFactory 未返回工厂".to_string())?
        };
        let d2d_device: ID2D1Device1 = d2d_factory
            .CreateDevice(&dxgi_device)
            .map_err(|error| format!("ID2D1Factory1::CreateDevice 失败: {error}"))?;
        let context = d2d_device
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
            .map_err(|error| format!("CreateDeviceContext 失败: {error}"))?;
        context.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        // 官方按 style.antialias_mode 设置文本抗锯齿；这里给默认值，接线时由 UIStyle 覆盖。
        context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_DEFAULT);

        // ⑥ 把交换链后缓冲包成 D2D 位图并设为绘制目标。
        let bitmap_properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let target_bitmap = context
            .CreateBitmapFromDxgiSurface(&surface, Some(&bitmap_properties))
            .map_err(|error| format!("CreateBitmapFromDxgiSurface 失败: {error}"))?;
        context.SetTarget(&target_bitmap);

        // ⑦ DirectComposition：把交换链挂到窗口的视觉上。
        let dcomp: IDCompositionDevice = unsafe {
            let mut dcomp: Option<IDCompositionDevice> = None;
            DCompositionCreateDevice(&dxgi_device, &IDCompositionDevice::IID, (&mut dcomp as *mut _).cast())
                .map_err(|error| format!("DCompositionCreateDevice 失败: {error}"))?;
            dcomp.ok_or_else(|| "DCompositionCreateDevice 未返回设备".to_string())?
        };
        let dcomp_target = dcomp
            .CreateTargetForHwnd(hwnd, true)
            .map_err(|error| format!("CreateTargetForHwnd 失败: {error}"))?;
        let visual = dcomp
            .CreateVisual()
            .map_err(|error| format!("CreateVisual 失败: {error}"))?;
        visual
            .SetContent(&swap_chain)
            .map_err(|error| format!("SetContent 失败: {error}"))?;
        dcomp_target
            .SetRoot(&visual)
            .map_err(|error| format!("SetRoot 失败: {error}"))?;
        dcomp
            .Commit()
            .map_err(|error| format!("Commit 失败: {error}"))?;

        Ok(Self {
            device,
            dxgi_device,
            factory,
            swap_chain,
            surface,
            d2d_device,
            context,
            target_bitmap,
            dcomp,
            dcomp_target,
            visual,
            width,
            height,
        })
    }

    pub(crate) fn device_context(&self) -> &ID2D1DeviceContext {
        &self.context
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 尺寸变化：官方在 `OnResize` 里释放目标位图、改交换链缓冲大小、再重建位图与目标。
    pub(crate) unsafe fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return Ok(());
        }
        self.context.SetTarget(None);
        self.swap_chain
            .ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, 0)
            .map_err(|error| format!("ResizeBuffers 失败: {error}"))?;
        let surface: IDXGISurface2 = self
            .swap_chain
            .GetBuffer(0)
            .map_err(|error| format!("GetBuffer(0) 失败: {error}"))?;
        let bitmap_properties = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: std::mem::ManuallyDrop::new(None),
        };
        let bitmap = self
            .context
            .CreateBitmapFromDxgiSurface(&surface, Some(&bitmap_properties))
            .map_err(|error| format!("重建目标位图失败: {error}"))?;
        self.context.SetTarget(&bitmap);
        self.surface = surface;
        self.target_bitmap = bitmap;
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// 每帧开始：官方在 `BeginDraw` 之前不做别的，清屏在绘制回调里。
    pub(crate) fn begin_draw(&self) {
        self.context.BeginDraw();
    }

    /// 每帧结束：EndDraw 后提交交换链。
    pub(crate) fn end_draw(&self) -> Result<(), String> {
        let mut tag1 = 0u64;
        let mut tag2 = 0u64;
        self.context
            .EndDraw(Some(&mut tag1), Some(&mut tag2))
            .map_err(|error| format!("EndDraw 失败: {error}"))?;
        self.swap_chain
            .Present(1, 0)
            .map_err(|error| format!("Present 失败: {error}"))
    }

    /// 全透明清屏：预乘 alpha 下 (0,0,0,0) 就是"这里什么都没有"，圆角外靠它透出背后。
    pub(crate) fn clear_transparent(&self) {
        self.context.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
    }
}
