//! D3D11 + DXGI 组合交换链 + DirectComposition + D2D 设备上下文的呈现层。
//!
//! 结构与候选窗的 `theme/src/presenter.rs` 同源（官方 `WeaselUI\d2d.cpp` 的
//! `D2D::InitDirect2D()` / `OnResize()`）：窗口用 `WS_EX_NOREDIRECTIONBITMAP`，
//! 交换链挂在 DirectComposition 视觉上，DWM 在窗口矩形背后做亚克力模糊，
//! 上方逐像素叠 D2D 画的半透明面板 —— 因此圆角之外能透出桌面。
//!
//! 与候选窗那份的差异：这里只需要「一个窗口一个目标」，不换主题、不做路径几何缓存，
//! 所以省掉 `d2d_factory()` 这类给主题绘制用的出口。
//!
//! 调用签名按实际生成的绑定核对过（`weasel-theme-support` 的 `d2d-bindings.rs`）：
//! `D2D1CreateFactory` 是泛型版、`EndDraw` 返回 `HRESULT`、`CreateSwapChainForComposition`
//! 是泛型版、`DCompositionCreateDevice` 也是泛型版。

#![allow(dead_code)]

use weasel_theme_support::d2d_bindings::*;
use windows_core::Interface;

/// 预乘 alpha 的全透明：表示「这里什么都没有」，让桌面透过。
pub type ColorF = D2D_COLOR_F;

pub const TRANSPARENT: ColorF = ColorF {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// 一次呈现所需的全部平台资源。必须由创建它们的 UI 线程释放。
pub struct Presenter {
    device: ID3D11Device,
    dxgi_device: IDXGIDevice,
    factory: IDXGIFactory2,
    swap_chain: IDXGISwapChain1,
    context: ID2D1DeviceContext1,
    target_bitmap: Option<ID2D1Bitmap1>,
    dcomp: IDCompositionDevice,
    dcomp_target: IDCompositionTarget,
    visual: IDCompositionVisual,
    brush: ID2D1SolidColorBrush,
    width: u32,
    height: u32,
}

/// 交换链与位图的公共描述（与候选窗同一组参数）。
fn swap_chain_description(width: u32, height: u32) -> DXGI_SWAP_CHAIN_DESC1 {
    DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE(DXGI_USAGE_RENDER_TARGET_OUTPUT),
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        Flags: 0,
    }
}

fn bitmap_properties() -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: core::mem::ManuallyDrop::new(None),
    }
}

impl Presenter {
    /// 按官方顺序建立全部资源：① D3D11 → ② DXGI 工厂 → ③ 组合交换链 →
    /// ④ 后缓冲 → ⑤ D2D 设备上下文 → ⑥ 位图目标 → ⑦ DirectComposition → ⑧ 画刷。
    pub unsafe fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("客户区尺寸为 0".into());
        }

        let mut device: Option<ID3D11Device> = None;
        let mut feature_level = D3D_FEATURE_LEVEL_11_0;
        let mut d3d_context: Option<ID3D11DeviceContext> = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT as u32,
                Some(&[
                    D3D_FEATURE_LEVEL_11_0,
                    D3D_FEATURE_LEVEL_10_1,
                    D3D_FEATURE_LEVEL_10_0,
                ]),
                D3D11_SDK_VERSION as u32,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut d3d_context),
            )
        }
        .ok()
        .map_err(|error| format!("D3D11CreateDevice 失败: {error}"))?;
        let device = device.ok_or_else(|| "D3D11CreateDevice 未返回设备".to_string())?;

        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|error| format!("ID3D11Device → IDXGIDevice 失败: {error}"))?;
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(0) }
            .map_err(|error| format!("CreateDXGIFactory2 失败: {error}"))?;

        let description = swap_chain_description(width, height);
        let swap_chain: IDXGISwapChain1 = unsafe {
            factory.CreateSwapChainForComposition(&dxgi_device, &description, None)
        }
        .map_err(|error| format!("CreateSwapChainForComposition 失败: {error}"))?;

        let d2d_factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
                .map_err(|error| format!("D2D1CreateFactory 失败: {error}"))?;
        let d2d_device_base: ID2D1Device = unsafe { d2d_factory.CreateDevice(&dxgi_device) }
            .map_err(|error| format!("ID2D1Factory1::CreateDevice 失败: {error}"))?;
        let d2d_device: ID2D1Device1 = d2d_device_base
            .cast()
            .map_err(|error| format!("ID2D1Device → ID2D1Device1 失败: {error}"))?;
        let context: ID2D1DeviceContext1 = unsafe {
            d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
        }
        .map_err(|error| format!("CreateDeviceContext 失败: {error}"))?;
        unsafe {
            context.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_DEFAULT);
        }

        let target_bitmap = unsafe { Self::make_target_bitmap(&context, &swap_chain)? };
        unsafe { context.SetTarget(&target_bitmap) };

        let dcomp: IDCompositionDevice = unsafe { DCompositionCreateDevice(&dxgi_device) }
            .map_err(|error| format!("DCompositionCreateDevice 失败: {error}"))?;
        let dcomp_target = unsafe { dcomp.CreateTargetForHwnd(hwnd, true) }
            .map_err(|error| format!("CreateTargetForHwnd 失败: {error}"))?;
        let visual = unsafe { dcomp.CreateVisual() }
            .map_err(|error| format!("CreateVisual 失败: {error}"))?;
        unsafe { visual.SetContent(&swap_chain) }
            .ok()
            .map_err(|error| format!("SetContent 失败: {error}"))?;
        unsafe { dcomp_target.SetRoot(&visual) }
            .ok()
            .map_err(|error| format!("SetRoot 失败: {error}"))?;
        unsafe { dcomp.Commit() }
            .ok()
            .map_err(|error| format!("Commit 失败: {error}"))?;

        let brush = unsafe { context.CreateSolidColorBrush(&ColorF::default(), None) }
            .map_err(|error| format!("CreateSolidColorBrush 失败: {error}"))?;

        Ok(Self {
            device,
            dxgi_device,
            factory,
            swap_chain,
            context,
            target_bitmap: Some(target_bitmap),
            dcomp,
            dcomp_target,
            visual,
            brush,
            width,
            height,
        })
    }

    unsafe fn make_target_bitmap(
        context: &ID2D1DeviceContext1,
        swap_chain: &IDXGISwapChain1,
    ) -> Result<ID2D1Bitmap1, String> {
        let base: IDXGISwapChain = swap_chain
            .cast()
            .map_err(|error| format!("IDXGISwapChain1 → IDXGISwapChain 失败: {error}"))?;
        let surface: IDXGISurface2 = unsafe { base.GetBuffer(0) }
            .map_err(|error| format!("GetBuffer(0) 失败: {error}"))?;
        let properties = bitmap_properties();
        unsafe { context.CreateBitmapFromDxgiSurface(&surface, Some(&properties)) }
            .map_err(|error| format!("CreateBitmapFromDxgiSurface 失败: {error}"))
    }

    /// 绘制句柄：`FillRoundedRectangle`、`DrawText`、`Clear` 都在基接口上。
    pub fn render_target(&self) -> ID2D1RenderTarget {
        self.context
            .cast()
            .expect("ID2D1DeviceContext1 必须能转换为 ID2D1RenderTarget")
    }

    pub fn brush(&self) -> &ID2D1SolidColorBrush {
        &self.brush
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 尺寸变化：先解除目标 → 丢掉旧位图 → 改缓冲大小 → 重建位图目标。
    pub unsafe fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return Ok(());
        }
        unsafe { self.context.SetTarget(None::<&ID2D1Image>) };
        // ResizeBuffers 要求后缓冲引用计数归零：只 SetTarget(None) 不够，必须把位图也丢掉。
        self.target_bitmap = None;
        let base: IDXGISwapChain = self
            .swap_chain
            .cast()
            .map_err(|error| format!("IDXGISwapChain1 → IDXGISwapChain 失败: {error}"))?;
        unsafe { base.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, 0) }
            .ok()
            .map_err(|error| format!("ResizeBuffers 失败: {error}"))?;
        let bitmap = unsafe { Self::make_target_bitmap(&self.context, &self.swap_chain)? };
        unsafe { self.context.SetTarget(&bitmap) };
        self.target_bitmap = Some(bitmap);
        self.width = width;
        self.height = height;
        Ok(())
    }

    pub fn begin_draw(&self) {
        unsafe { self.context.BeginDraw() };
    }

    /// 每帧结束：EndDraw 检查 HRESULT，然后**必须** Present 才会显示。
    pub fn end_draw(&self) -> Result<(), String> {
        unsafe { self.context.EndDraw(None, None) }
            .ok()
            .map_err(|error| format!("EndDraw 失败: {error}"))?;
        let base: IDXGISwapChain = self
            .swap_chain
            .cast()
            .map_err(|error| format!("IDXGISwapChain1 → IDXGISwapChain 失败: {error}"))?;
        unsafe { base.Present(1, 0) }
            .ok()
            .map_err(|error| format!("Present 失败: {error}"))
    }

    pub fn clear_transparent(&self) {
        unsafe { self.context.Clear(Some(&TRANSPARENT)) };
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        // 不摘掉窗口的合成目标，下一次对同一 HWND 调 CreateTargetForHwnd 会
        // 返回 DCOMPOSITION_ERROR_WINDOW_ALREADY_COMPOSED。
        unsafe {
            let _ = self.dcomp_target.SetRoot(None::<&IDCompositionVisual>);
        }
    }
}
