//! official 主题 DLL 入口 —— 照 fxliang/weasel 的 WeaselUI：D3D11 + DXGI 交换链 + DirectComposition + D2D1.1；DWM 圆角区域模糊 + 冰蓝玻璃色板；原生窗口和图形资源始终由创建它们的 UI 线程管理。
//!
//! 本 crate 复用公共主题 API 与 Windows/Direct2D 支持层，并导出内部主题工厂。
pub use weasel_theme_api as theme_api;
pub use weasel_theme_support::{appearance, bindings, d2d_bindings, diagnostics, presentation};
#[path = "mod.rs"]
mod theme;
weasel_theme_api::export_theme!(theme::Factory);

// 与官方 WeaselUI 的呈现层移植已写好，见 src\presenter.rs；