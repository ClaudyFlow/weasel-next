//! 配置描述仅启动时读取；主题选择只改变单页分区显隐。
#![windows_subsystem = "windows"]
mod backdrop;
mod bindings;
mod broker;
mod colors;
mod dialogs;
mod style_keys;
mod switcher_dialog;
mod skins;
mod frame;
mod updater;
mod wasm_modules;
// 框架无关的设置核心来自 weasel_settings 库
// （配置文档读写与冲突检测、主题目录发现、表单字段模型、应用选项编辑）。
use weasel_settings::{app_options, catalog, config, form};
use slint::{ComponentHandle, Model, VecModel};
use std::{cell::RefCell, collections::HashSet, rc::Rc};
use weasel_common::process::RuntimePaths;
slint::include_modules!();

/// 设置窗口的运行时模型；仅在 Slint 事件循环线程内访问。
///
/// `document` 保存基础配置与用户覆盖，`fields`/`owners` 将 UI 行映射回目录分区；
/// `changed` 用于限定保存前的分区校验，`invalid` 和 `module_errors` 阻止保存。
struct State {
    /// 已加载的配置文档，包含基础值及待保存的用户补丁。
    document: config::Document,
    /// 配置项目录及其分区元数据。
    catalog: catalog::Catalog,
    /// 与完整 UI 行模型按索引对应的编辑字段。
    fields: Vec<form::Field>,
    /// 每个字段所属的目录分区索引，与 `fields` 同步增长。
    owners: Vec<usize>,
    /// 自上次成功保存以来发生改动的分区索引。
    changed: HashSet<usize>,
    /// 当前无效的字段索引；存在时禁止保存。
    invalid: HashSet<usize>,
    /// 以“模块 ID/键”标识的无效模块编辑。
    module_errors: HashSet<String>,
    /// 最近成功读取的自动更新设置；读取失败时为 `None`，此项不可编辑。
    auto_update_saved: Option<bool>,
    /// 界面当前选择的自动更新值，成功写入后同步到 `auto_update_saved`。
    auto_update_pending: bool,
}
impl State {
    /// 根据有效配置重建应用选项行，并保留已有行的展开状态。
    ///
    /// 遇到非对象配置、超过 256 项或无效布尔值时返回错误，不提交部分模型。
    fn show_apps(&self, ui: &SettingsWindow) -> Result<(), String> {
        let effective = self.document.effective();
        let snapshot = weasel_common::settings::ConfigSnapshot::new(effective.clone());
        let Some(items) = effective["app_options"].as_object() else {
            if effective["app_options"].is_null() {
                ui.set_apps(Default::default());
                return Ok(());
            }
            return Err("app_options 必须为对象".into());
        };
        if items.len() > 256 {
            return Err("应用项最多 256 个".into());
        }
        let old = ui.get_apps();
        let expanded: HashSet<String> = (0..old.row_count())
            .filter_map(|i| old.row_data(i))
            .filter(|r| r.expanded)
            .map(|r| r.name.to_string())
            .collect();
        let mut rows = Vec::new();
        for (name, config) in items {
            if !config.is_object() {
                return Err(format!("app_options.{name} 必须为对象"));
            }
            let patch = &self.document.patch["app_options"][name];
            let mut options = Vec::new();
            for (key, title) in [
                ("ascii_mode", "默认英文模式"),
                ("inline_preedit", "内联预编辑"),
            ] {
                let checked = snapshot
                    .app_bool(name, key)
                    .map_err(|error| format!("{name}.{key}: {error}"))?;
                options.push(AppOption {
                    key: key.into(),
                    title: title.into(),
                    checked,
                    overridden: patch.get(key).is_some(),
                    description: format!(
                        "{}\n{}",
                        if patch.get(key).is_some() {
                            "用户指定；恢复继承会清除此覆盖。"
                        } else if config.get(key).is_some() {
                            "程序配置已指定此应用选项。"
                        } else {
                            "未指定：继承全局设置，随全局设置变化。"
                        },
                        if key == "ascii_mode" {
                            "首次激活时使用英文模式；启用共享中英文状态时以共享状态为准。"
                        } else {
                            "在应用文本框内显示预编辑；关闭时由支持此功能的主题显示。"
                        }
                    )
                    .into(),
                });
            }
            rows.push(AppEntry {
                name: name.clone().into(),
                expanded: expanded.contains(name),
                overridden: self.document.patch["app_options"].get(name).is_some(),
                options: Rc::new(VecModel::from(options)).into(),
            });
        }
        ui.set_apps(Rc::new(VecModel::from(rows)).into());
        Ok(())
    }
    /// 判断目录分区是否匹配当前主题及可选的 WASM 主题挂载条件。
    fn visible(&self, index: usize) -> bool {
        let value = self.document.effective();
        let mount = &self.catalog.sections[index].mount;
        mount.is_empty()
            || (value["theme"].as_str() == mount.get(1).map(String::as_str)
                && (mount.len() < 5
                    || value["themeSettings"]["wasm"]["theme"].as_str()
                        == mount.get(3).map(String::as_str)))
    }
    /// 按目录生成完整字段模型和过滤后的可见模型，并同步应用及 WASM 模块行。
    ///
    /// 字段、所有者映射与 UI 源索引保持同序。此方法会向字段和所有者列表追加内容，
    /// 因此应在新建状态时调用；模型生成失败时返回错误。
    fn show(&mut self, ui: &SettingsWindow) -> Result<(), String> {
        let mut rows = Vec::new();
        for (index, section) in self.catalog.sections.iter().enumerate() {
            for (offset, field) in form::fields(section, &self.document)?
                .into_iter()
                .enumerate()
            {
                rows.push(SettingField {
                    source_index: self.fields.len() as i32,
                    section: if offset == 0 {
                        section.title.clone().into()
                    } else {
                        "".into()
                    },
                    shown: self.visible(index),
                    title: field.title.clone().into(),
                    key: field.key.clone().into(),
                    description: field.description.clone().into(),
                    choices: Rc::new(VecModel::from(
                        field
                            .choices
                            .iter()
                            .skip(1)
                            .map(|s| s.clone().into())
                            .collect::<Vec<_>>(),
                    ))
                    .into(),
                    selected: field.display_selected(),
                    checked: field.display_selected() == 1,
                    overridden: field.selected != 0,
                    boolean: field.boolean,
                    text: field.text.clone().into(),
                    text_input: field.kind != "choice" && field.kind != "enum_number",
                    enum_number: field.kind == "enum_number",
                    color_input: field.kind == "color",
                    allow_system_color: field.allow_system_color,
                    adaptive_color: field.adaptive_color,
                    multiline: field.multiline,
                });
                self.fields.push(field);
                self.owners.push(index);
            }
        }
        let model = Rc::new(VecModel::from(rows));
        // 不向布局传递隐藏行，避免空 VerticalBox 的默认 padding/spacing 累积。
        // 保留源索引，过滤后的行号不能用作编辑草稿的索引。
        ui.set_visible_fields(
            Rc::new(slint::FilterModel::new(
                model.clone(),
                |row: &SettingField| row.shown,
            ))
            .into(),
        );
        ui.set_fields(model.into());
        self.show_apps(ui)?;
        wasm_modules::show(self, ui)?;
        Ok(())
    }
    /// 原位更新字段行的可见标志，不重建模型或改变编辑索引。
    fn update_visibility(&self, ui: &SettingsWindow) {
        let model = ui.get_fields();
        for (index, &owner) in self.owners.iter().enumerate() {
            if let Some(mut row) = model.row_data(index) {
                let shown = self.visible(owner);
                if row.shown != shown {
                    row.shown = shown;
                    model.set_row_data(index, row);
                }
            }
        }
    }
}
/// 初始化原生窗口后启动 Slint 事件循环；配置目录在后台线程读取。
///
/// 所有 UI 状态和回调均留在事件循环线程，后台任务通过容量为 1 的通道回传结果。
/// 返回窗口初始化、线程创建或事件循环产生的错误。
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 官方习惯：命令行参数选择要打开的对话框（/style、/switcher、/dict）。
    // 命中时只弹对话框，不启动 Slint 主界面。
    {
        // 不带参数时默认打开官方的「方案选单设定」：先选方案，再选皮肤。
    // 旧的 Slint 界面容易被误认为"设置还是旧版"，退居二线：需要它时传 /legacy。
    let args: Vec<String> = {
        // （/legacy 由下面的分支识别：命中时跳过对话框，直接进 Slint 界面）
        let raw: Vec<String> = std::env::args().skip(1).collect();
        if raw.is_empty() {
            vec!["/switcher".to_string()]
        } else {
            raw
        }
    };
        let script_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        // 三个官方对话框（/switcher、/style、/dict）都用它找出厂数据；
        // 与皮肤页共用同一套查找，避免"从这里跑有数据、从那里跑没数据"。
        let probe = script_dir.join("theme").join("default").join("default.theme.toml");
        let shared = skins::resolve_shared_dir(&script_dir, &probe);
        let user = std::env::var_os("APPDATA")
            .map(|appdata| std::path::PathBuf::from(appdata).join("Rime-Next"))
            .unwrap_or_default();
        if dialogs::run_from_args(&args, &shared, &user) {
            return Ok(());
        }
    }    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("skia".into())
        .with_winit_window_attributes_hook(|attributes| {
            use slint::winit_030::winit::platform::windows::{
                BackdropType, WindowAttributesExtWindows,
            };
            // 在 Winit 创建 HWND 时设置材质，早于渲染表面初始化。
            attributes
                .with_enabled_buttons(
                    slint::winit_030::winit::window::WindowButtons::MINIMIZE
                        | slint::winit_030::winit::window::WindowButtons::CLOSE,
                )
                .with_transparent(backdrop::supported())
                .with_system_backdrop(if backdrop::supported() {
                    BackdropType::MainWindow
                } else {
                    BackdropType::None
                })
        })
        .select()?;
    let window = SettingsWindow::new()?;
    wire_skins(&window);
    // 首帧就以透明背景初始化渲染表面；apply 失败时恢复纯色。
    window.set_mica_enabled(backdrop::supported());
    colors::install(&window);
    let paths = RuntimePaths::discover()?;
    window.set_config_path(
        paths
            .user_data
            .join("weasel.custom.toml")
            .display()
            .to_string()
            .into(),
    );
    let state = Rc::new(RefCell::new(None::<State>));
    {
        let state = state.clone();
        let weak = window.as_weak();
        window.on_changed(move |index, selected, text| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut guard = state.borrow_mut();
            let Some(state) = guard.as_mut() else {
                return;
            };
            let Some(field) = state.fields.get_mut(index as usize) else {
                return;
            };
            field.selected = selected;
            field.text = text.to_string();
            let result = field
                .value(selected, text.as_str())
                .and_then(|value| form::assign(&mut state.document.patch, &field.path, value));
            if selected == 0 && result.is_ok() {
                field.text = field.inherited_text.clone();
            }
            state.changed.insert(state.owners[index as usize]);
            let model = ui.get_fields();
            if let Some(mut row) = model.row_data(index as usize) {
                row.selected = field.display_selected();
                row.checked = field.display_selected() == 1;
                row.overridden = selected != 0;
                row.text = field.text.clone().into();
                model.set_row_data(index as usize, row);
            }
            ui.set_dirty(true);
            match result {
                Ok(()) => {
                    state.invalid.remove(&(index as usize));
                    state.update_visibility(&ui);
                    if let Err(error) = state.show_apps(&ui) {
                        ui.set_status(error.into());
                    }
                    if let Err(error) = wasm_modules::show(state, &ui) {
                        ui.set_status(error.into());
                    }
                    ui.set_status("有未保存修改；可仅保存或保存并应用。".into());
                }
                Err(error) => {
                    state.invalid.insert(index as usize);
                    ui.set_status(format!("字段无效：{error}").into());
                }
            }
            ui.set_invalid(!state.invalid.is_empty() || !state.module_errors.is_empty());
        });
    }
    {
        let state = state.clone();
        let weak = window.as_weak();
        window.on_update_changed(move |checked| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut guard = state.borrow_mut();
            let Some(state) = guard.as_mut() else {
                return;
            };
            if let Some(saved) = state.auto_update_saved {
                state.auto_update_pending = checked;
                let dirty = !state.changed.is_empty() || checked != saved;
                ui.set_dirty(dirty);
                ui.set_status(if dirty {
                    "有未保存修改；可仅保存或保存并应用。".into()
                } else {
                    "自动检查更新设置未更改。".into()
                });
            }
        });
    }
    {
        let state = state.clone();
        let weak = window.as_weak();
        let update_directory = paths.executable_directory.clone();
        window.on_save(move |apply| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let mut guard = state.borrow_mut();
            let Some(state) = guard.as_mut() else {
                return;
            };
            if !state.invalid.is_empty() || !state.module_errors.is_empty() {
                return;
            }
            for &index in &state.changed {
                let section = &state.catalog.sections[index];
                if let Err(error) = section
                    .metadata
                    .validate(&form::effective(section, &state.document))
                {
                    ui.set_status(format!("{}：{error}", section.title).into());
                    return;
                }
            }
            let config_changed = !state.changed.is_empty();
            let update_changed = state
                .auto_update_saved
                .is_some_and(|saved| saved != state.auto_update_pending);
            if config_changed {
                if let Err(error) = state.document.save() {
                    ui.set_status(format!("保存失败：{error}").into());
                    return;
                }
                state.changed.clear();
            }
            if update_changed {
                if let Err(error) = updater::write(&update_directory, state.auto_update_pending) {
                    ui.set_dirty(true);
                    let prefix = if config_changed {
                        "普通配置已保存，但"
                    } else {
                        ""
                    };
                    ui.set_status(format!("{prefix}自动检查更新设置保存失败：{error}").into());
                    return;
                }
                state.auto_update_saved = Some(state.auto_update_pending);
            }
            ui.set_dirty(false);
            let mut messages = Vec::new();
            // 现有重启命令只重启子组件，不重新初始化 broker 中的 WinSparkle。
            if apply && (config_changed || !update_changed) {
                messages.push(match broker::request_restart() {
                    Ok(()) => "已向 broker 发送重启服务请求。".to_owned(),
                    Err(error) => format!("未能请求应用普通配置：{error}"),
                });
            } else if config_changed {
                messages.push("普通配置已保存，尚未请求应用。".to_owned());
            }
            if update_changed {
                messages.push(
                    "自动检查更新设置已保存；退出并重新启动托盘中的算法服务后生效。".to_owned(),
                );
            }
            if messages.is_empty() {
                messages.push("没有需要保存的修改。".to_owned());
            }
            ui.set_status(messages.join("\n").into());
        });
    }
    window.set_status("正在读取配置描述…".into());
    {
        let state = state.clone();
        let weak = window.as_weak();
        window.on_module_action(move |kind, id, key, value| {
            let Some(ui) = weak.upgrade() else {
                return "设置窗口不可用".into();
            };
            let mut guard = state.borrow_mut();
            let Some(state) = guard.as_mut() else {
                return "配置尚未加载".into();
            };
            let error_key = format!("{id}/{key}");
            if let Err(error) = wasm_modules::action(state, &kind, &id, &key, &value) {
                if kind == "set" {
                    state.module_errors.insert(error_key);
                    ui.set_invalid(true);
                }
                return error.into();
            }
            if kind == "remove" {
                state
                    .module_errors
                    .retain(|k| !k.starts_with(&format!("{id}/")));
            } else {
                state.module_errors.remove(&error_key);
            }
            ui.set_invalid(!state.invalid.is_empty() || !state.module_errors.is_empty());
            state.changed.insert(0);
            ui.set_dirty(true);
            // 使用模块同时同步原有 theme 字段及主题分区，不重新读取描述。
            let effective = state.document.effective();
            for (i, field) in state.fields.iter_mut().enumerate() {
                if kind == "remove"
                    && field.path.len() >= 5
                    && field.path[..3] == ["themeSettings", "wasm", "modules"]
                    && field.path[3] == id.as_str()
                {
                    field.selected = 0;
                    field.text = field.inherited_text.clone();
                    state.invalid.remove(&i);
                    if let Some(mut row) = ui.get_fields().row_data(i) {
                        row.text = field.text.clone().into();
                        row.selected = field.display_selected();
                        row.checked = field.display_selected() == 1;
                        row.overridden = false;
                        ui.get_fields().set_row_data(i, row);
                    }
                }
                if kind == "select" && field.path == ["themeSettings", "wasm", "theme"] {
                    field.text = effective["themeSettings"]["wasm"]["theme"]
                        .as_str()
                        .unwrap_or("")
                        .into();
                    field.selected = 1;
                    if let Some(mut row) = ui.get_fields().row_data(i) {
                        row.text = field.text.clone().into();
                        row.overridden = true;
                        ui.get_fields().set_row_data(i, row);
                    }
                }
            }
            state.update_visibility(&ui);
            ui.set_invalid(!state.invalid.is_empty() || !state.module_errors.is_empty());
            if let Err(error) = wasm_modules::show(state, &ui) {
                return error.into();
            }
            ui.set_status("有未保存修改；新增模块或变更路径后需重新打开设置加载描述。".into());
            "".into()
        });
    }
    {
        let state = state.clone();
        let weak = window.as_weak();
        window.on_app_add(move |name| {
            let Some(ui) = weak.upgrade() else {
                return false;
            };
            if let Some(state) = state.borrow_mut().as_mut() {
                let result = app_options::add(&mut state.document, name.as_str());
                if let Err(error) = result {
                    ui.set_app_add_error(error.into());
                    return false;
                }
                ui.set_app_add_error("".into());
                let added = true;
                finish_app_edit(state, &ui, Ok(()));
                if added {
                    let model = ui.get_apps();
                    for i in 0..model.row_count() {
                        if let Some(mut row) = model.row_data(i) {
                            if row.name.as_str() == name.trim() {
                                row.expanded = true;
                                model.set_row_data(i, row);
                                break;
                            }
                        }
                    }
                }
                return added;
            }
            false
        });
    }
    {
        let state = state.clone();
        let weak = window.as_weak();
        window.on_app_remove(move |name| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Some(state) = state.borrow_mut().as_mut() {
                let result = app_options::remove(&mut state.document, name.as_str());
                finish_app_edit(state, &ui, result);
            }
        });
    }
    {
        let state = state.clone();
        let weak = window.as_weak();
        window.on_app_edit(move |name, key, value, reset| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if let Some(state) = state.borrow_mut().as_mut() {
                let result = app_options::edit(
                    &mut state.document,
                    name.as_str(),
                    key.as_str(),
                    if reset { None } else { Some(value) },
                );
                finish_app_edit(state, &ui, result);
            }
        });
    }
    let weak = window.as_weak();
    let (tx, rx) = std::sync::mpsc::sync_channel::<
        Result<(config::Document, catalog::Catalog, Result<bool, String>), String>,
    >(1);
    window.on_loaded(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let Ok(result) = rx.try_recv() else {
            return;
        };
        match result {
            Ok((document, catalog, auto_update)) => {
                let (auto_update_saved, update_status) = match auto_update {
                    Ok(enabled) => (Some(enabled), String::new()),
                    Err(error) => (None, format!("更新组件不可用：{error}")),
                };
                let mut next = State {
                    document,
                    catalog,
                    fields: vec![],
                    owners: vec![],
                    changed: HashSet::new(),
                    invalid: HashSet::new(),
                    module_errors: HashSet::new(),
                    auto_update_saved,
                    auto_update_pending: auto_update_saved.unwrap_or(false),
                };
                if let Err(error) = next.show(&ui) {
                    ui.set_status(error.into());
                    return;
                }
                ui.set_status(next.catalog.notices.join("\n").into());
                ui.set_update_available(auto_update_saved.is_some());
                ui.set_update_checked(next.auto_update_pending);
                ui.set_update_status(update_status.into());
                *state.borrow_mut() = Some(next);
                ui.set_editable(true);
            }
            Err(error) => ui.set_status(format!("加载失败：{error}").into()),
        }
    });
    let weak = window.as_weak();
    if let Err(error) = std::thread::Builder::new()
        .name("settings-catalog".into())
        .spawn(move || {
            let result = config::Document::load(&paths).and_then(|document| {
                catalog::load(&paths, &document).map(|catalog| {
                    let auto_update = updater::read(&paths.executable_directory);
                    (document, catalog, auto_update)
                })
            });
            if tx.send(result).is_ok() {
                let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_loaded());
            }
        })
    {
        window.set_status(format!("无法启动读取任务：{error}").into());
    }
    window.on_request_close(|| {
        let _ = slint::quit_event_loop();
    });
    window.show()?;
    backdrop::apply(&window);
    slint::run_event_loop()?;
    Ok(())
}

/// 收尾应用选项编辑：成功时标记配置已修改并刷新行，失败时展示原始错误。
fn finish_app_edit(state: &mut State, ui: &SettingsWindow, result: Result<(), String>) {
    match result {
        Ok(()) => {
            state.changed.insert(0);
            ui.set_dirty(true);
            match state.show_apps(ui) {
                Ok(()) => ui.set_status(
                    "有未保存修改。清除应用覆盖后，安装配置中的应用项仍会保留。".into(),
                ),
                Err(error) => ui.set_status(error.into()),
            }
        }
        Err(error) => ui.set_status(error.into()),
    }
}
// ---------------------------------------------------------------- 皮肤页接线

/// 皮肤页与数据层的接线：读官方皮肤文件、把配色喂给界面、把选择写回补丁。
///
/// 路径约定与 librime 一致：出厂文件在程序文件夹的 `Rime-Next\weasel.toml`，
/// 用户补丁在 `%APPDATA%\Rime-Next\weasel.custom.toml`。界面只负责显示与回调。
fn wire_skins(ui: &SettingsWindow) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let script_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    // 共用同一套查找：exe 旁边 → 上溯找 Rime-Next → %ProgramFiles%。
    // 只看 exe 旁边的话，从构建目录跑会全部落空（列表空得和故障一样）。
    let probe = script_dir.join("theme").join("default").join("default.theme.toml");
    let shared = skins::resolve_shared_dir(&script_dir, &probe);
    let user = std::env::var_os("APPDATA")
        .map(|appdata| std::path::PathBuf::from(appdata).join("Rime-Next"))
        .unwrap_or_default();

    let skins = Rc::new(RefCell::new(skins::Skins::load(&shared, &user)));
    push_skins(ui, &skins.borrow());

    // 重新读取：用户在别处改过配置时手动刷新
    {
        let ui_weak = ui.as_weak();
        let skins = skins.clone();
        ui.on_scheme_refresh(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            skins.borrow_mut().reload();
            push_skins(&ui, &skins.borrow());
        });
    }

    // 选中一套配色：写补丁 → 重读 → 刷新界面，并把结果写进状态栏
    {
        let ui_weak = ui.as_weak();
        let skins = skins.clone();
        ui.on_scheme_picked(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            match skins.borrow_mut().select(id.as_str()) {
                Ok(message) => {
                    ui.set_status(message.into());
                }
                Err(error) => {
                    ui.set_status(error.into());
                }
            }
            push_skins(&ui, &skins.borrow());
        });
    }
}

/// 把数据层的状态推给界面：行模型、当前配色、状态栏、预览颜色。
fn push_skins(ui: &SettingsWindow, skins: &skins::Skins) {
    use slint::{Model, VecModel};
    use std::rc::Rc;

    let rows = skins.rows();
    // 当前配色的预览颜色取自它自己的行（找不到就退回默认色）
    let active = skins.active_id();
    let current = rows
        .iter()
        .find(|row| row.id == active.as_str())
        .cloned();
    ui.set_schemes(Rc::new(VecModel::from(rows)).into());
    ui.set_active_scheme(active.as_str().into());
    ui.set_scheme_status(skins.status().into());
    if let Some(row) = current {
        ui.set_scheme_preview_back(row.back_color);
        ui.set_scheme_preview_border(row.border_color);
        ui.set_scheme_preview_text(row.text_color);
        ui.set_scheme_preview_hilited_back(row.hilited_back_color);
        ui.set_scheme_preview_hilited_text(row.hilited_text_color);
        ui.set_scheme_preview_comment(row.comment_color);
    }
}
/// 打开「输入统计」窗口。`/stat` 走这里。
///
/// 数据来自 `%APPDATA%\Rime-Next\stat\stat.bin`（TIP 采集，见 `Tip/src/stats.rs`；
/// 旧的 `stats\*.toml` 只读兜底）。读文件/算数字都在 `weasel_settings::stats` 里，
/// 这里只做「数据 → Slint 属性」的搬运。
pub fn show_stats() -> Result<(), String> {
    let ui = StatsWindow::new().map_err(|error| error.to_string())?;
    let weak = ui.as_weak();

    // 刷新按钮和首次填充走同一条路径：重读文件、重算、重灌。数据量是「一天一条记录」，
    // 直接全量重读即可，不必增量。
    let apply: Rc<dyn Fn()> = Rc::new(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let (entries, mut warnings) = weasel_settings::stats::load();
        let snapshot =
            weasel_settings::stats::snapshot(&entries, weasel_settings::stats::today_local());

        let (title, level, exp_cur, exp_max) = career_level(snapshot.total_exp);
        ui.set_level_title(title.into());
        ui.set_level(level as i32);
        ui.set_exp_cur(exp_cur as i32);
        ui.set_exp_max(exp_max as i32);
        ui.set_exp_ratio(if exp_max > 0 {
            exp_cur as f32 / exp_max as f32
        } else {
            0.0
        });
        ui.set_exp_next(
            format!("还差 {} 升下一级", exp_max.saturating_sub(exp_cur)).into(),
        );
        ui.set_realm(career_realm(snapshot.total_chars).into());

        ui.set_today_keys(snapshot.today_keys.min(i32::MAX as u64) as i32);
        ui.set_today_chars(snapshot.today_chars.min(i32::MAX as u64) as i32);
        ui.set_today_speed(snapshot.today_speed as i32);
        ui.set_streak_days(snapshot.streak_days as i32);

        // GitHub 贡献图是 **7 行（星期）× 5 列（周）= 35 格**，
        // Slint 里按 `week * 7 + day` 索引，所以 `snapshot.days` 恒为 35 个
        // （未采集的格子 count = 0，不是"不填"）。
        let day_cells: Vec<StatDay> = snapshot
            .days
            .iter()
            .map(|cell| StatDay {
                count: cell.count as i32,
                tip: cell.tip.as_str().into(),
                today: cell.today,
            })
            .collect();
        ui.set_days(slint::ModelRc::new(slint::VecModel::from(day_cells)));

        let words: Vec<StatWord> = snapshot
            .words
            .iter()
            .map(|(word, count)| StatWord {
                word: word.as_str().into(),
                count: (*count).min(i32::MAX as u64) as i32,
            })
            .collect();
        ui.set_words(slint::ModelRc::new(slint::VecModel::from(words)));

        // 没有数据 ≠ 出错：给一句灰字解释，别让用户以为窗口坏了。
        ui.set_hint(
            if snapshot.has_data {
                ""
            } else {
                "还没有采集到数据：替换掉 TIP（Weasel-Next.dll）后打的字才会开始累计。"
            }
            .into(),
        );
        if !warnings.is_empty() {
            for warning in &warnings {
                eprintln!("Weasel-Next-Setting: 统计文件 {warning}");
            }
            let first = warnings.remove(0);
            ui.set_status(
                format!("{} 个统计文件读不出来（例：{first}）", warnings.len() + 1).into(),
            );
        } else {
            ui.set_status("".into());
        }
    });

    apply();
    {
        let apply = apply.clone();
        ui.on_refresh(move || apply());
    }
    ui.show().map_err(|error| error.to_string())?;
    // 窗口要等事件循环跑起来才有真实尺寸/位置（`show()` 只是投递请求），
    // 所以「立刻摆一次」之外，再在 120ms 后补一次 —— 这次才是真正生效的那次。
    place_window_in_work_area(ui.window());
    {
        let weak = ui.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(120), move || {
            if let Some(ui) = weak.upgrade() {
                place_window_in_work_area(ui.window());
            }
        });
    }
    ui.run().map_err(|error| error.to_string())
}

/// 把窗口摆进主屏工作区（居中，放不下就贴左上）。
///
/// 为什么需要：Windows 对同一窗口类的**新建窗口会级联偏移**，实测 `/stat` 连续打开几次就会
/// 漂到 (302,224) —— 740 逻辑高 × 125% 缩放 = 925 物理，底边直接跑到 1080 屏幕外，
/// 用户看到的是「致谢那行被切掉」的半截窗口（2026-10-17 截图复现）。显式定位一次即可。
fn place_window_in_work_area(window: &slint::Window) {
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    unsafe extern "system" {
        /// `SPI_GETWORKAREA` = 0x0030，取主屏去掉任务栏后的区域（物理像素）。
        fn SystemParametersInfoW(action: u32, param: u32, out: *mut Rect, win_ini: u32) -> i32;
    }
    let mut work = Rect::default();
    if unsafe { SystemParametersInfoW(0x0030, 0, &mut work, 0) } == 0 {
        return;
    }
    let size = window.size();
    let (width, height) = (size.width as i32, size.height as i32);
    let (work_width, work_height) = (work.right - work.left, work.bottom - work.top);
    if width <= 0 || height <= 0 || work_width <= 0 || work_height <= 0 {
        return;
    }
    let x = work.left + (work_width - width).max(0) / 2;
    let y = work.top + (work_height - height).max(0) / 2;
    window.set_position(slint::PhysicalPosition::new(x, y));
}
/// 触发一次部署，并显示进度窗口。
///
/// 部署由 **broker** 执行（引擎单实例，Setting 自己拉引擎只会得到 `AlreadyRunning`），
/// 所以这里没有回调可用，只能：
///   1. 记下部署前 `user.yaml` 的 `var/last_build_time`
///   2. 给 broker 发 `WM_COMMAND(1001)`
///   3. 起一个 `slint::Timer` 轮询那个时间戳，变了 = 部署完成
///   4. 期间按"阶段"推进进度条（纯估算，但足以让用户知道没卡死）
pub fn deploy_with_progress() -> Result<(), String> {
    let ui = DeployProgressWindow::new().map_err(|error| error.to_string())?;
    let weak = ui.as_weak();

    // 轮询用的状态：已经跑了几轮、部署前的 last_build_time
    let before = read_last_build_time();
    let ticks = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let finished_flag = std::rc::Rc::new(std::cell::Cell::new(false));
    let finished_for_timer = finished_flag.clone();
    let ticks_for_timer = ticks.clone();

    // 立刻发部署命令
    let _ = std::panic::catch_unwind(|| crate::switcher_dialog::request_deploy());

    let timer = slint::Timer::default();
    let weak_for_timer = weak.clone();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(250),
        move || {
            if finished_for_timer.get() { return; }
            let Some(ui) = weak_for_timer.upgrade() else { return };
            let n = ticks_for_timer.get() + 1;
            ticks_for_timer.set(n);

            // 阶段文案 + 估算进度（部署典型耗时 4~8 秒）
            let (stage, progress) = match n {
                1..=4 => ("正在停止输入法进程…", 0.05 + n as f32 * 0.02),
                5..=16 => ("正在编译输入方案（librime）…", 0.15 + (n - 4) as f32 * 0.035),
                17..=24 => ("正在同步程序文件…", 0.60 + (n - 16) as f32 * 0.03),
                25..=32 => ("正在重启输入法…", 0.85 + (n - 24) as f32 * 0.01),
                _ => ("等待部署完成…", 0.95),
            };
            ui.set_stage(stage.into());
            ui.set_progress(progress.min(0.99));
            ui.set_detail(format!("已等待 {:.1} 秒", n as f32 * 0.25).into());

            // 完成判据：last_build_time 变了（rime 部署成功会写它）
            let now = read_last_build_time();
            let done = match (before, now) {
                (Some(a), Some(b)) => b > a,
                (None, Some(_)) => true,
                _ => false,
            };
            // 超时保护：20 秒还没变就认为失败，别让窗口挂死
            let timed_out = n >= 80;
            if done || timed_out {
                finished_for_timer.set(true);
                ui.set_finished(done && !timed_out);
                ui.set_failed(timed_out);
                ui.set_progress(if done { 1.0 } else { ui.get_progress() });
                ui.set_stage(if done { "完成" } else { "超时（可能仍在后台部署，稍后看候选窗是否恢复）" }.into());
                ui.set_detail("".into());
                // 完成后再撑 0.8 秒让用户看见，然后自动关闭
                let weak_done = weak_for_timer.clone();
                let closer = slint::Timer::default();
                closer.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_millis(1600),
                    move || {
                        if let Some(ui) = weak_done.upgrade() {
                            let _ = ui.hide();
                            slint::quit_event_loop();
                        }
                    },
                );
                std::mem::forget(closer);
            }
        },
    );
    // timer 要活到窗口关掉
    std::mem::forget(timer);

    let weak_cancel = weak.clone();
    ui.on_cancel(move || {
        if let Some(ui) = weak_cancel.upgrade() {
            let _ = ui.hide();
        }
        slint::quit_event_loop();
    });

    ui.run().map_err(|error| error.to_string())
}

/// 读 `user.yaml` 里的 `var/last_build_time`（rime 部署成功会更新它）。
fn read_last_build_time() -> Option<i64> {
    let path = std::env::var_os("APPDATA")?;
    let path = std::path::PathBuf::from(path).join("Rime-Next").join("user.yaml");
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("last_build_time:") {
            return value.trim().parse::<i64>().ok();
        }
    }
    None
}
// ── 生涯段位（移植自 input_statistics.lua 的 career_levels / career_realms）──────
//
// 20 级、按累计经验（≈上屏字数）分段；7 境界按累计字数。这两张表和 lua 插件
// **完全一致**，否则同一个用户在两处看到的段位会不一样。
const CAREER_LEVELS: &[(u32, u64, &str)] = &[
    (1, 0, "🌱 初识键盘"),
    (2, 100, "🍃 摸键学徒"),
    (3, 300, "🌿 码字新人"),
    (4, 600, "🌾 指尖练习生"),
    (5, 1_000, "🌼 输入学徒"),
    (6, 2_000, "🌸 码流行者"),
    (7, 4_000, "🌺 飞键使"),
    (8, 7_000, "🌻 连击者"),
    (9, 12_000, "🌷 码阵构筑师"),
    (10, 20_000, "🌹 熟练输入师"),
    (11, 35_000, "⚔️ 码道武者"),
    (12, 55_000, "🗡️ 飞码剑士"),
    (13, 80_000, "🔥 码力掌控者"),
    (14, 120_000, "🌪️ 疾风键客"),
    (15, 180_000, "✨ 码道宗师"),
    (16, 280_000, "🌌 星码使"),
    (17, 420_000, "🌠 破空键圣"),
    (18, 620_000, "👑 万码之王"),
    (19, 900_000, "🪽 羽化码仙"),
    (20, 1_300_000, "🐉 键道真君"),
];

const CAREER_REALMS: &[(u64, &str)] = &[
    (10_000_000, "码仙境"),
    (5_000_000, "码圣境"),
    (1_000_000, "通玄境"),
    (200_000, "御码境"),
    (50_000, "飞键境"),
    (10_000, "熟键境"),
    (0, "凡键境"),
];

/// 按累计经验算段位。返回 (称号, 等级, 本级已得经验, 本级所需经验)。
fn career_level(total_exp: u64) -> (&'static str, u32, u64, u64) {
    for (index, (lv, start, title)) in CAREER_LEVELS.iter().enumerate().rev() {
        if total_exp >= *start {
            let end = CAREER_LEVELS
                .get(index + 1)
                .map(|next| next.1)
                .unwrap_or(start + 500_000);
            return (title, *lv, total_exp - start, end - start);
        }
    }
    let first = CAREER_LEVELS[0];
    let second = CAREER_LEVELS[1];
    (first.2, first.0, total_exp, second.1 - first.1)
}

/// 按累计字数算境界。
fn career_realm(total_chars: u64) -> &'static str {
    for (base, name) in CAREER_REALMS {
        if total_chars >= *base {
            return name;
        }
    }
    "凡键境"
}
