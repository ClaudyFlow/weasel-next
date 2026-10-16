//! 照官方 WeaselUI 搬过来的呈现层：D3D11 + DXGI 交换链 + DirectComposition + D2D 设备上下文。
//!
//! 对应官方 `WeaselUI\d2d.cpp` 的 `D2D::InitDirect2D()`（第 155-267 行）与 `D2D::OnResize()`。
//!
//! 本文件里的调用签名是照 **实际生成的绑定**（`target\release\build\weasel-theme-support-*\out\d2d-bindings.rs`）
//! 逐个核对后修正的，修正点记录如下，便于以后升级 windows-rs 时对照：
//!
//! 1. `D2D1CreateFactory` 是泛型版：`D2D1CreateFactory::<ID2D1Factory1>(factorytype, options)`，
//!    不再传 `riid` 指针。
//! 2. 颜色结构体叫 **`D2D_COLOR_F`**（不是 `D2D1_COLOR_F`），并且已从过滤清单里剔除，
//!    这里用 `D2D1_COLOR_F` 等价的自定义结构体替代（见 [`ColorF`]）——两者内存布局相同
//!    （r, g, b, a 各 f32）。
//! 3. `ID2D1RenderTarget::EndDraw` 返回 `HRESULT`（不是 `Result`，也不接 tag 指针），
//!    所以用 `.ok()?` 转换。
//! 4. `ID2D1Factory1::CreateDevice` 返回 `ID2D1Device`，需要再 `cast::<ID2D1Device1>()`。
//! 5. `ID2D1Device::CreateDeviceContext` 返回 `ID2D1DeviceContext1`（比 `ID2D1DeviceContext` 更高版本）。
//! 6. `IDXGIFactory2::CreateSwapChainForComposition` 是泛型版：
//!    `(pdevice, *const DXGI_SWAP_CHAIN_DESC1, prestricttooutput)`，限制输出传 `None`。
//! 7. `IDXGIAdapter::GetParent` 不在生成集里，改用独立函数 `CreateDXGIFactory2`。
//! 8. `DCompositionCreateDevice` 是泛型版：`DCompositionCreateDevice::<_, IDCompositionDevice>(dxgi_device)`。

#![allow(dead_code)]

use crate::d2d_bindings::*;
use windows_core::Interface;

/// 颜色用绑定里的 `D3DCOLORVALUE`（`ID2D1RenderTarget::Clear` 要的就是它）。
pub(crate) type ColorF = D3DCOLORVALUE;

/// 全透明：预乘 alpha 下表示"这里什么都没有"。
pub(crate) const TRANSPARENT: ColorF = ColorF { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

/// 一次呈现所需的全部平台资源。必须由创建它们的 UI 线程释放。
pub(crate) struct Presenter {
    device: ID3D11Device,
    dxgi_device: IDXGIDevice,
    factory: IDXGIFactory2,
    swap_chain: IDXGISwapChain1,
    d2d_device: ID2D1Device1,
    context: ID2D1DeviceContext1,
    target_bitmap: Option<ID2D1Bitmap1>,
    dcomp: IDCompositionDevice,
    dcomp_target: IDCompositionTarget,
    visual: IDCompositionVisual,
    brush: ID2D1SolidColorBrush,
    width: u32,
    height: u32,
}

/// 交换链与位图的公共描述（官方参数原样照抄）。
fn swap_chain_description(width: u32, height: u32) -> DXGI_SWAP_CHAIN_DESC1 {
    DXGI_SWAP_CHAIN_DESC1 {
        Width: width,
        Height: height,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
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
    /// 按官方顺序建立全部资源：① D3D11 → ② DXGI 工厂 → ③ 组合交换链 → ④ 后缓冲 →
    /// ⑤ D2D 设备上下文 → ⑥ 位图目标 → ⑦ DirectComposition → ⑧ 画刷。
    pub(crate) unsafe fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("客户区尺寸为 0（官方此处直接返回）".into());
        }

        // ① D3D11 设备：必须带 BGRA 支持，D2D 才能与之互操作。
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

        // ② 取 DXGI 设备；工厂用独立函数创建（官方走 DeviceResources，我们用 CreateDXGIFactory2）。
        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|error| format!("ID3D11Device → IDXGIDevice 失败: {error}"))?;
        let factory: IDXGIFactory2 = unsafe { CreateDXGIFactory2(0) }
            .map_err(|error| format!("CreateDXGIFactory2 失败: {error}"))?;

        // ③ 组合用交换链。
        let description = swap_chain_description(width, height);
        let swap_chain: IDXGISwapChain1 = unsafe {
            factory.CreateSwapChainForComposition(&dxgi_device, &description, None)
        }
        .map_err(|error| format!("CreateSwapChainForComposition 失败: {error}"))?;

        // ⑤ D2D 设备与设备上下文（先建设备，后建设备上下文）。
        let d2d_factory: ID2D1Factory1 = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
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

        // ④⑥ 取后缓冲并包成 D2D 位图，设为绘制目标。
        let target_bitmap = unsafe { Self::make_target_bitmap(&context, &swap_chain)? };
        unsafe { context.SetTarget(&target_bitmap) };

        // ⑦ DirectComposition：交换链挂到窗口视觉上。
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

        // ⑧ 一支实色画刷（颜色每帧由 SetColor 改）。
        let brush = unsafe { context.CreateSolidColorBrush(&ColorF::default(), None) }
            .map_err(|error| format!("CreateSolidColorBrush 失败: {error}"))?;

        Ok(Self {
            device,
            dxgi_device,
            factory,
            swap_chain,
            d2d_device,
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

    /// 取后缓冲并建成 D2D 位图目标（官方 GetBuffer → CreateBitmapFromDxgiSurface）。
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

    /// 绘图句柄：`FillGeometry`、`DrawText`、`Clear`、`SetDpi` 等都在基接口 `ID2D1RenderTarget` 上，
    /// 而设备上下文是它的派生接口，所以这里做一次转换交给绘制代码使用。
    pub(crate) fn render_target(&self) -> ID2D1RenderTarget {
        self.context
            .cast()
            .expect("ID2D1DeviceContext1 必须能转换为 ID2D1RenderTarget")
    }
    pub(crate) fn device_context(&self) -> &ID2D1DeviceContext1 {
        &self.context
    }

    pub(crate) fn brush(&self) -> &ID2D1SolidColorBrush {
        &self.brush
    }

    pub(crate) fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 尺寸变化：与官方 OnResize 一致 —— 先解除目标，再改缓冲大小，最后重建位图与目标。
    pub(crate) unsafe fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return Ok(());
        }
        unsafe { self.context.SetTarget(None::<&ID2D1Image>) };
        // ★ ResizeBuffers 要求后缓冲的引用计数归零：SetTarget(None) 只解除"目标"，
        // 旧位图本身仍持有后缓冲 → 必须真正丢掉它，否则返回 DXGI_ERROR_INVALID_CALL (0x887A0001)。
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

    /// 每帧开始绘制。
    pub(crate) fn begin_draw(&self) {
        unsafe { self.context.BeginDraw() };
    }

    /// 每帧结束：EndDraw 检查 HRESULT，然后提交交换链。
    pub(crate) fn end_draw(&self) -> Result<(), String> {
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

    /// 全透明清屏：圆角之外靠它透出背后。
    pub(crate) fn clear_transparent(&self) {
        unsafe { self.context.Clear(Some(&TRANSPARENT)) };
    }
}

    /// 销毁：**先把窗口的合成目标摘掉**。
    ///
    /// 不摘就丢的话，下一次对同一个 HWND 调 `CreateTargetForHwnd` 会返回
    /// `DCOMPOSITION_ERROR_WINDOW_ALREADY_COMPOSED (0x88980800)` —— 窗口"已经被合成过了"。
    /// 渲染器在设备恢复时会反复重建 Presenter，所以这一步不能省。
    impl Drop for Presenter {
        fn drop(&mut self) {
            unsafe {
                let _ = self.dcomp_target.SetRoot(None::<&IDCompositionVisual>);
            }
        }
    }
