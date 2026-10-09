//! weasel-settings-tauri 的 Rust 后端。
//!
//! 全部设置逻辑都在 `weasel_settings::session::Session`（与 Slint 版共用的框架无关核心）：
//! 加载配置与主题目录、生成 schema 表单字段、校验、原子保存、外部修改检测、按应用覆盖。
//! 本文件只负责：加锁取出会话 → 调用 → 把视图（`View`，已派生 `Serialize`）交给前端。
//!
//! 因此前端只做渲染与交互，不需要知道任何配置细节；两个设置界面对同一份配置的行为一致。
//!
//! 单元测试放在 `weasel-settings` 库里（`session` 模块）：本 crate 依赖 Tauri/tao/WebView2，
//! 在无应用清单的测试宿主里会在加载期失败（`STATUS_ENTRYPOINT_NOT_FOUND`），不适合跑测试。

use std::sync::Mutex;

use serde::Serialize;
use tauri::State;
use weasel_common::process::RuntimePaths;
use weasel_settings::session::{Session, View};

/// Tauri 管理的全局状态；`None` 表示尚未加载配置。
#[derive(Default)]
struct AppState(Mutex<Option<Session>>);

fn discover() -> Result<RuntimePaths, String> {
    RuntimePaths::discover().map_err(|e| format!("无法确定运行时路径：{e}"))
}

/// 取出会话执行操作；未加载时给出明确提示。
fn with_session<T>(
    state: &State<'_, AppState>,
    action: impl FnOnce(&mut Session, &RuntimePaths) -> Result<T, String>,
) -> Result<T, String> {
    let paths = discover()?;
    let mut guard = state.0.lock().map_err(|_| "设置状态已损坏")?;
    let session = guard.as_mut().ok_or("设置尚未加载")?;
    action(session, &paths)
}

/// 重新打开会话并返回视图（加载/丢弃改动用）。
fn open_and_store(state: &State<'_, AppState>) -> Result<View, String> {
    let paths = discover()?;
    paths
        .ensure()
        .map_err(|e| format!("无法创建运行目录：{e}"))?;
    let session = Session::open(&paths)?;
    let view = session.view(&paths)?;
    *state.0.lock().map_err(|_| "设置状态已损坏")? = Some(session);
    Ok(view)
}

/// 加载（或重新加载）配置并返回完整视图。
#[tauri::command]
fn settings_load(state: State<'_, AppState>) -> Result<View, String> {
    open_and_store(&state)
}

/// 从当前会话重建视图，不访问磁盘。
#[tauri::command]
fn settings_view(state: State<'_, AppState>) -> Result<View, String> {
    with_session(&state, |session, paths| session.view(paths))
}

/// 应用一个字段改动（写入内存补丁），返回刷新后的视图。
#[tauri::command]
fn settings_apply(
    state: State<'_, AppState>,
    section: usize,
    field: usize,
    selected: i32,
    text: String,
) -> Result<View, String> {
    with_session(&state, |session, paths| {
        session.apply(section, field, selected, &text)?;
        session.view(paths)
    })
}

/// 校验并原子保存用户补丁。
#[tauri::command]
fn settings_save(state: State<'_, AppState>) -> Result<View, String> {
    with_session(&state, |session, paths| {
        session.save()?;
        session.view(paths)
    })
}

/// 丢弃内存改动：从磁盘重新加载。
#[tauri::command]
fn settings_discard(state: State<'_, AppState>) -> Result<View, String> {
    open_and_store(&state)
}

/// 清空用户补丁（恢复全部默认值；仍需保存才写盘）。
#[tauri::command]
fn settings_reset(state: State<'_, AppState>) -> Result<View, String> {
    with_session(&state, |session, paths| {
        session.reset()?;
        session.view(paths)
    })
}

/// 新增一个按应用覆盖项。
#[tauri::command]
fn settings_app_add(state: State<'_, AppState>, app: String) -> Result<View, String> {
    with_session(&state, |session, paths| {
        session.app_add(&app)?;
        session.view(paths)
    })
}

/// 删除一个按应用覆盖项。
#[tauri::command]
fn settings_app_remove(state: State<'_, AppState>, app: String) -> Result<View, String> {
    with_session(&state, |session, paths| {
        session.app_remove(&app)?;
        session.view(paths)
    })
}

/// 修改某个应用的受支持选项；`value` 为 `None` 时移除该覆盖。
#[tauri::command]
fn settings_app_set(
    state: State<'_, AppState>,
    app: String,
    key: String,
    value: Option<bool>,
) -> Result<View, String> {
    with_session(&state, |session, paths| {
        session.app_set(&app, &key, value)?;
        session.view(paths)
    })
}

/// 兼容早期前端的握手命令。
#[derive(Serialize)]
struct PingReply {
    msg: String,
    backend: &'static str,
}

#[tauri::command]
fn weasel_ping() -> PingReply {
    PingReply {
        msg: "ok".into(),
        backend: "weasel-settings-tauri",
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            weasel_ping,
            settings_load,
            settings_view,
            settings_apply,
            settings_save,
            settings_discard,
            settings_reset,
            settings_app_add,
            settings_app_remove,
            settings_app_set
        ])
        .run(tauri::generate_context!())
        .expect("error while running weasel-settings-tauri");
}
