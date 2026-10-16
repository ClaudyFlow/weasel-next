//! Windows 渲染器程序入口：区分主题预览与实时呈现并启动相应服务。
//!
//! 进程实例按运行模式分别互斥。
#![windows_subsystem = "windows"]

use weasel_theme_api as theme_api;
use weasel_theme_support::{appearance, bindings, d2d_bindings, presentation};

mod diagnostics;
mod notifications;
mod preview;
mod preview_window;
mod rpc;
mod state;
mod theme_adapter;
mod theme_dll;
mod ui_runtime;

/// 解析启动参数、取得模式专属进程实例并运行预览或实时渲染服务。
fn main() {
    // [排障] 全局 panic 钩子：把 panic 的**消息与源码位置**写进渲染器日志。
    //
    // 之前 `catch_unwind` 把载荷丢掉了 ✗，日志里只剩一句"theme operation panicked"，
    // 完全没法定位（"界面消失"查了几轮都卡在这里 ✓）。这个钩子不改行为，只补证据。
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<非字符串载荷>".to_owned());
        let location = info
            .location()
            .map(|place| format!("{}:{}", place.file(), place.line()))
            .unwrap_or_else(|| "<未知位置>".to_owned());
        diagnostics::record(format_args!("PANIC: {message}（位置 {location}）"));
    }));

    let preview = match weasel_common::service_owner::take_launch_args(std::env::args_os().skip(1))
        .map_err(|error| error.to_string())
        .and_then(preview::parse_args)
    {
        Ok(preview) => preview,
        Err(error) => {
            eprintln!("Weasel-Next-Renderer: {error}");
            std::process::exit(1);
        }
    };
    let component = if preview != theme_api::UiMode::Live {
        "renderer-preview"
    } else {
        "renderer"
    };
    let _instance = match weasel_common::process::SingleInstance::acquire(component) {
        Ok(instance) => instance,
        Err(error) => {
            eprintln!("Weasel-Next-Renderer: {error}");
            return;
        }
    };
    diagnostics::initialize();
    diagnostics::record(format_args!("starting (mode={preview:?})"));
    let result = if preview != theme_api::UiMode::Live {
        preview::run(preview)
    } else {
        rpc::run()
    };
    if let Err(error) = result {
        diagnostics::record(format_args!("{error}"));
        std::process::exit(1);
    }
    diagnostics::record(format_args!("stopped"));
}
