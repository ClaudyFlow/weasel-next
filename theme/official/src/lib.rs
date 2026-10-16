//! official 主题 DLL 入口 —— 照 fxliang/weasel 的 `WeaselUI`：D3D11 + DXGI 组合交换链 +
//! DirectComposition + Direct2D 1.1，配置读 `theme\theme.toml` 与
//! `theme\<皮肤>\<皮肤>.theme.toml`（出厂皮肤是官方 `weasel.toml` 的 36 套配色）。
//!
//! 模块分工：`config` 负责官方 style 语义（默认值、夹取、DPI 缩放、配色），`layout`
//! 是官方 `DoLayout` 的移植，`paint` 是官方 `DoPaint`/`_HighlightRect` 的移植，
//! `mod.rs` 管窗口、消息、定位与主题接口，`presenter` 是设备与合成层。
//!
//! 原生窗口和图形资源始终由创建它们的 UI 线程管理。
pub use weasel_theme_api as theme_api;
pub use weasel_theme_support::{appearance, bindings, d2d_bindings, diagnostics, presentation};
#[path = "mod.rs"]
mod theme;
weasel_theme_api::export_theme!(theme::Factory);

mod presenter;