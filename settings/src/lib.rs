//! 框架无关的设置核心。
//!
//! 该库被两个前端共用：
//! * `weasel-settings`（Slint 界面，本包的 bin，需 `gui` 特性）；
//! * `weasel-settings-tauri`（Tauri 界面，以 `default-features = false` 依赖本库）。
//!
//! 因此这里只放与界面框架无关的逻辑——配置文档读写与冲突检测、主题目录发现、schema
//! 元数据校验、表单字段模型、按应用选项编辑；任何 Slint/Tauri 类型都不应出现在本库。
pub mod app_options;
pub mod catalog;
pub mod config;
pub mod form;
pub mod metadata;
pub mod session;
