//! Shared messages and RPC transport for weasel-next.

pub mod about;
#[cfg(windows)]
pub mod bindings;
pub mod command_menu;
#[cfg(windows)]
pub mod comrt;
pub mod data_frame;
pub mod deploy_manifest;
pub mod eassy;
pub mod rime_layout;
pub mod deploy_protocol;
pub mod logging;
pub mod message;
#[cfg(windows)]
pub mod process;
pub mod rpc;
#[cfg(windows)]
pub mod service_owner;
pub mod settings;
#[cfg(windows)]
pub mod task_dialog;
#[cfg(feature = "wasm-metadata")]
pub mod wasm_metadata;
pub mod windows_security;
pub mod yaml;
