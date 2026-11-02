//! 配置描述仅启动时读取；主题选择只改变单页分区显隐。
#![windows_subsystem = "windows"]
mod backdrop;
mod bindings;
mod broker;
mod colors;
mod dialogs;
mod style_keys;
mod switcher_dialog;
mod single_instance;
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

/// 用户文件夹：默认 `%APPDATA%\Rime-Next`；安装时自选的绝对路径（程序目录里的
/// `userdir.txt`）优先，环境变量 `WEASEL_NEXT_USER_DIR` 最高。
///
/// 以前这里有 5 处各自 `std::env::var_os("APPDATA")` 拼 `Rime-Next`，等于把"用户文件夹"
/// 写死在代码里 —— 安装器就算给了选择页也无处落地。统一走 common 的解析。
fn user_data_dir() -> std::path::PathBuf {
    weasel_common::process::user_data_dir().unwrap_or_default()
}

/// 统计页自动刷新的轮询间隔（用户 2026-10-26：**5 秒**，原 1.5 秒）。
///
/// 定时器只比 `stat.bin` 的 (mtime, size)，没变化就直接返回，所以间隔只决定
/// "数字最迟多久自己变"，几乎不带来开销；主窗口与 `/stat` 窗口共用这一个值。
const STAT_REFRESH_POLL: std::time::Duration = std::time::Duration::from_millis(5_000);

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
                // `ascii_mode` 的全局键就在宿主配置里，`app_bool` 能回退到它；
                // `inline_preedit` 的全局值却是**主题文件**的 `style/inline_preedit`
                // （见 `weasel_style::theme::inline_preedit`），宿主配置里没有这个键。
                // 之前两个都用 `app_bool`，于是每个带 app_options 的配置都会在这里抛
                // "missing required configuration value inline_preedit" → `show()`
                // 整个失败 → 主界面停在只读（2026-10-17 用户看到的就是这个）。
                let checked = match key {
                    "inline_preedit" => snapshot
                        .app_option_bool(name, key)
                        .map_err(|error| format!("{name}.{key}: {error}"))?
                        // 未指定时跟随主题；主题默认关闭，这里按"未勾选"显示，
                        // 下面的说明写清"继承主题"，别让勾选框冒充真值。
                        .unwrap_or(false),
                    _ => snapshot
                        .app_bool(name, key)
                        .map_err(|error| format!("{name}.{key}: {error}"))?,
                };
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
                        } else if key == "inline_preedit" {
                            // 这个键的全局值不在宿主配置里，不能写"继承全局设置"。
                            "未指定：跟随主题的 style/inline_preedit（主题默认关闭）。"
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
                    // 分区序号：Slint 侧只有分区首行带 section 名，按 tab 过滤必须靠它。
                    category: index as i32,
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
        refresh_page_fields(ui);
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
        refresh_page_fields(ui);
    }
}

/// 该页签是否显示这一行字段（按目录分区序号 `category` 分派）。
///
/// 页签只有三页有字段：3 常规 / 4 更新（更新页只有固定的「其他」块，没有字段）。
/// 「应用选项」（app_options）与「外观与行为」（主题分区）两页已按用户要求删除，
/// 所以这两类字段不再出现在任何页签里 —— 数据层和编辑逻辑都还在，只是没有入口。
fn page_shows(page: i32, row: &SettingField) -> bool {
    match page {
        3 => row.category == 0 && row.key != "app_options",
        _ => false,
    }
}

/// 重算当前页签的字段模型 `page-fields`。
///
/// 不能让 Slint 在 `for` 里用 `if` 过滤：`for` 每轮都会生成一个元素，被过滤掉的那行
/// 虽然高 0，父布局的 spacing 仍按行数累加 —— 内容少的页会白裂出两三百像素空白
/// （2026-10-18 实测）。在这里筛好模型，界面上就没有空行。
fn refresh_page_fields(ui: &SettingsWindow) {
    let page = ui.get_current_page();
    let model = ui.get_fields();
    let rows: Vec<SettingField> = (0..model.row_count())
        .filter_map(|index| model.row_data(index))
        .filter(|row| row.shown && page_shows(page, row))
        .collect();
    ui.set_page_fields(Rc::new(VecModel::from(rows)).into());
}
/// 初始化原生窗口后启动 Slint 事件循环；配置目录在后台线程读取。
///
/// 所有 UI 状态和回调均留在事件循环线程，后台任务通过容量为 1 的通道回传结果。
/// 返回窗口初始化、线程创建或事件循环产生的错误。
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 设置界面唯一化（用户 2026-10-28：不许同时存在多个设置界面）。
    // 只锁**无参数**的主界面启动；带参数的对话框（/style、/switcher、/dict、/stat）
    // 不占这把锁 —— 否则设置窗开着时，托盘的「方案选单 / 界面风格」会全都打不开。
    // 第二个主界面实例不建窗口：把已有窗口拉到前台后直接退出。
    let _settings_instance = if std::env::args_os().len() <= 1 {
        match weasel_common::process::SingleInstance::acquire("settings-ui") {
            Ok(guard) => Some(guard),
            Err(weasel_common::process::SingleInstanceError::AlreadyRunning) => {
                single_instance::focus_existing();
                return Ok(());
            }
            Err(_) => None,
        }
    } else {
        None
    };

    // 官方习惯：命令行参数选择要打开的对话框（/style、/switcher、/dict）。
    // 命中时只弹对话框，不启动 Slint 主界面；**不带参数 = Slint 主界面**。
    //
    // ️ 这里曾经写成「无参数就换成 /switcher」，与 `Server/src/menu_actions.rs`
    // 的约定正好相反 —— 那里「设置」故意不传参数，指望落到主界面。结果托盘点
    // 「设置」弹出来的是方案选单对话框，用户 2026-10-17 21:11 报过
    // 「设置界面没显示，直接是这个」。别再改回去。
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let script_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        // 三个官方对话框（/switcher、/style、/dict）都用它找出厂数据；
        // 与皮肤页共用同一套查找，避免"从这里跑有数据、从那里跑没数据"。
        let probe = script_dir.join("theme").join("default").join("default.theme.yaml");
        let shared = skins::resolve_shared_dir(&script_dir, &probe);
        let user = user_data_dir();
        if dialogs::run_from_args(&args, &shared, &user) {
            return Ok(());
        }
    }
    slint::BackendSelector::new()
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
    // 设置主页 = 个人记录：先把采集数据灌进内嵌的 StatsPage，
    // 之后由下面那个 5 秒的定时器自动更新（用户 2026-10-26：自动刷新，刷新按钮去掉）。
    apply_stats_to_settings(&window);
    wire_skins(&window);
    wire_schemas(&window);
    wire_dicts(&window);
    let download_loader = wire_downloads(&window);
    window.invoke_dict_refresh();
    {
        // 切页签时重算当前页的字段模型（筛选放在 Rust 侧，界面上不留空行）。
        // 同时：切到「下载」页（idx 6）时自动刷新仓库列表。
        let weak = window.as_weak();
        let loader = std::sync::Arc::clone(&download_loader);
        window.on_page_selected(move |page| {
            if let Some(window) = weak.upgrade() {
                refresh_page_fields(&window);
                if page == 6 {
                    loader();
                }
            }
        });
    }
    wire_official_dialogs(&window);
    // 首帧就以透明背景初始化渲染表面；apply 失败时恢复纯色。
    window.set_mica_enabled(backdrop::supported());
    colors::install(&window);
    let paths = RuntimePaths::discover()?;
    // 用户 2026-10-18：界面上不再显示 `%APPDATA%\Rime-Next\weasel.custom.toml` 那行路径。
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
    // 主窗口也必须显式摆位：Windows 对**同一窗口类的新窗口是级联偏移**的 —— 每开一次就往
    // 右下挪 32px，开几次整块跑出屏幕（2026-10-26 连开三次实测：(270,192)、(302,224)、(334,256)，
    // 全部超出 1920×1080 工作区，右下角看不见）。`/stat` 窗口早就这么做了，主窗口一直漏了。
    // `show()` 只是投递请求，尺寸/缩放在首帧之后才是真的 ⇒ 立刻摆一次，120ms 后再摆一次。
    place_window_in_work_area(window.window());
    {
        // 120ms 时窗口还没缩到最终尺寸（实测那一刻 window.size() 是 1474×853 逻辑，最终 1294×720），
        // 拿它算会顶到工作区左上角；所以再补一次 400ms 之后的，那时尺寸已稳定、能真正居中。
        for delay in [120u64, 400u64] {
            let weak = window.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
                if let Some(window) = weak.upgrade() {
                    place_window_in_work_area(window.window());
                }
            });
        }
    }
    // ── 自动刷新（用户 2026-10-26：「能不能做成自动刷新」）──
    // 每 [`STAT_REFRESH_POLL`]（5 秒，用户同日改的间隔）看一次 stat.bin 的修改时间/大小：
    // 变过才重读重算，没变直接返回。
    // 定时器必须活到事件循环结束（Slint 的 Timer 一 drop 就停），所以声明在 main 的作用域里。
    let auto_refresh = slint::Timer::default();
    {
        let weak = window.as_weak();
        let stamp = Rc::new(RefCell::new(stat_stamp()));
        auto_refresh.start(
            slint::TimerMode::Repeated,
            STAT_REFRESH_POLL,
            move || {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let now = stat_stamp();
                if now.is_none() || *stamp.borrow() == now {
                    return;
                }
                *stamp.borrow_mut() = now;
                apply_stats_to_settings(&window);
            },
        );
    }
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
/// 路径约定：出厂皮肤是**主题链**（程序文件夹的
/// `Rime-Next\theme\<皮肤>\<皮肤>.theme.yaml`），用户补丁在
/// `%APPDATA%\Rime-Next\weasel.yaml`（用户目录根，YAML）。界面只负责显示与回调。
fn wire_skins(ui: &SettingsWindow) {
    use std::cell::RefCell;
    use std::rc::Rc;

    let script_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    // 共用同一套查找：exe 旁边 → 上溯找 Rime-Next → %ProgramFiles%。
    // 只看 exe 旁边的话，从构建目录跑会全部落空（列表空得和故障一样）。
    let probe = script_dir.join("theme").join("default").join("default.theme.yaml");
    let shared = skins::resolve_shared_dir(&script_dir, &probe);
    let user = user_data_dir();

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

    // 候选窗材质三档（2026-11-02 用户：一个 DLL、在设置里切）。
    // 只写 `theme/material.txt`：主题侧 `theme/src/paint.rs::tier` 运行时读它，
    // 而且 material.txt 进了配置时间戳 ⇒ 打完下一次按键就重载，不用重启输入法。
    ui.set_candidate_material(weasel_settings::material::current(&user).min(2) as i32);
    {
        let ui_weak = ui.as_weak();
        let user = user.clone();
        ui.on_material_picked(move |index| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let index = (index.max(0) as usize).min(weasel_settings::material::LABELS.len() - 1);
            match weasel_settings::material::set(&user, index) {
                Ok(_) => ui.set_material_status(
                    format!(
                        "候选窗材质 → {}（打一次字生效）",
                        weasel_settings::material::LABELS[index]
                    )
                    .into(),
                ),
                Err(error) => ui.set_material_status(format!("写材质失败：{error}").into()),
            }
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
// ------------------------------------------------------- 方案选单页（tab 2）

/// 方案选单页：读可用方案 → 勾选 → 写 `schema/schema.yaml` → 请 broker 重新部署。
///
/// 数据层与官方 `/switcher` 对话框是同一份（`weasel_style::switcher`），落盘也走
/// `write_schema_ids`（保留原有的 `active`），所以两条路径不会互相踩。官方对话框
/// 里的快捷键设置这页不做，留一个「官方对话框…」按钮跳过去。
fn wire_schemas(ui: &SettingsWindow) {
    let script_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    let probe = script_dir.join("theme").join("default").join("default.theme.yaml");
    let shared = skins::resolve_shared_dir(&script_dir, &probe);
    let user = user_data_dir();
    // 首次运行把出厂 `default.yaml` 的 schema_list 抄进用户目录那份 schema.yaml
    // （官方对话框也这样起手）；漏了这一步界面会显示"一个都没勾"。
    let _ = weasel_style::switcher::seed_schema_table(&shared, &user);

    let rows = Rc::new(RefCell::new(Vec::<weasel_style::switcher::SchemaInfo>::new()));
    reload_schemas(ui, &shared, &user, &rows);

    // 勾选/取消：只改内存，点「保存并部署」才写盘（与官方"确定才生效"一致）。
    // 树结构：点系列标题折叠/展开（用户 2026-10-19）
    {
        let ui_weak = ui.as_weak();
        let shared = shared.to_path_buf();
        let user = user.to_path_buf();
        let rows = Rc::clone(&rows);
        ui.on_schema_series_select(move |name| {
            toggle_series(name.as_str());
            let Some(ui) = ui_weak.upgrade() else { return };
            push_schemas(&ui, &shared, &user, &rows);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let rows = rows.clone();
        let shared = shared.to_path_buf();
        let user = user.to_path_buf();
        ui.on_schema_toggle(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            {
                let mut rows = rows.borrow_mut();
                if let Some(row) = rows.iter_mut().find(|row| row.id == id.as_str()) {
                    row.selected = !row.selected;
                }
            }
            push_schemas(&ui, &shared, &user, &rows);
        });

    }
    {
        let ui_weak = ui.as_weak();
        let rows = rows.clone();
        let user = user.clone();
        ui.on_schema_apply(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let ids: Vec<String> = rows
                .borrow()
                .iter()
                .filter(|row| row.selected)
                .map(|row| row.id.clone())
                .collect();
            // 官方 OnOK：一项都没选就报错，不写文件。
            if ids.is_empty() {
                ui.set_schema_status("至少要选用一项。".into());
                return;
            }
            let path = user
                .join("schema")
                .join(weasel_style::switcher::SCHEMA_TABLE_FILE);
            match weasel_style::switcher::write_schema_ids(&path, &ids) {
                Ok(()) => {
                    ui.set_schema_status(
                        format!("已保存 {} 个方案，正在请 broker 重新部署…", ids.len()).into(),
                    );
                    // 部署必须由掌握引擎生死的 broker 做（引擎单实例，自己拉第二个只会
                    // 得到 AlreadyRunning）；与官方对话框 OnOK 之后是同一个动作。
                    switcher_dialog::request_deploy();
                }
                Err(error) => {
                    ui.set_schema_status(format!("写入方案选单失败：{error}").into());
                }
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let rows = rows.clone();
        let shared = shared.to_path_buf();
        let user = user.to_path_buf();
        ui.on_schema_refresh(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            reload_schemas(&ui, &shared, &user, &rows);
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_schema_open_dialog(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Err(error) = open_official("/switcher") {
                ui.set_schema_status(format!("无法打开方案选单：{error}").into());
            }
        });
    }
}

/// 重新读一遍可用方案（失败只更新状态栏，不清空已有列表）。
fn reload_schemas(
    ui: &SettingsWindow,
    shared: &std::path::Path,
    user: &std::path::Path,
    rows: &Rc<RefCell<Vec<weasel_style::switcher::SchemaInfo>>>,
) {
    match weasel_style::switcher::list_schemas(shared, user) {
        Ok(list) => *rows.borrow_mut() = list,
        Err(error) => ui.set_schema_status(format!("读取方案列表失败：{error}").into()),
    }
    push_schemas(ui, shared, user, rows);
}

/// 左侧导航当前选中的系列（None = 全部）。
/// 树里被折叠的系列（VS Code 资源管理器式）。
static COLLAPSED: std::sync::Mutex<Option<std::collections::BTreeSet<String>>> =
    std::sync::Mutex::new(None);

fn collapsed_snapshot() -> std::collections::BTreeSet<String> {
    match COLLAPSED.lock() {
        Ok(guard) => guard.clone().unwrap_or_default(),
        Err(poisoned) => poisoned.into_inner().clone().unwrap_or_default(),
    }
}

fn toggle_series(name: &str) {
    let mut guard = match COLLAPSED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let set = guard.get_or_insert_with(Default::default);
    if !set.remove(name) {
        set.insert(name.to_string());
    }
}


/// 把方案列表推给界面（当前方案从 `schema.yaml` 的 `active` 读）。
fn push_schemas(
    ui: &SettingsWindow,
    shared: &std::path::Path,
    user: &std::path::Path,
    rows: &Rc<RefCell<Vec<weasel_style::switcher::SchemaInfo>>>,
) {
    let active = weasel_style::switcher::read_schema_table(shared, user).active;
    let rows = rows.borrow();
    let selected = rows.iter().filter(|row| row.selected).count();
    // 用户 2026-10-19：按系列分组（schema\<系列>\<方案>\）。
    // 同一系列的方案前面插一行标题（header = true），界面按 header 渲染成灰字标题。
    // 用户 2026-10-19：VS Code 文件夹视图 —— 系列是可折叠的文件夹节点，方案缩进。
    let collapsed = collapsed_snapshot();
    let mut model: Vec<SchemaRow> = Vec::with_capacity(rows.len() + 4);
    let mut current_series: Option<String> = None;
    let mut series_open = true;
    for row in rows.iter() {
        let series = row.series.clone().unwrap_or_default();
        if current_series.as_deref() != Some(series.as_str()) {
            if !series.is_empty() {
                series_open = !collapsed.contains(&series);
                let chevron = if series_open { "▾  " } else { "▸  " };
                let count = rows
                    .iter()
                    .filter(|other| other.series.as_deref() == Some(series.as_str()))
                    .count();
                model.push(SchemaRow {
                    id: series.clone().into(),
                    name: "".into(),
                    author: "".into(),
                    description: "".into(),
                    selected: false,
                    active: false,
                    series: format!("{chevron}{series}  ({count})").into(),
                    header: true,
                    level: 0,
                });
            } else {
                series_open = true;
            }
            current_series = Some(series.clone());
        }
        if !series.is_empty() && !series_open {
            continue;
        }
        model.push(SchemaRow {
            id: row.id.as_str().into(),
            name: row.name.as_str().into(),
            author: row.author.as_str().into(),
            description: row
                .description
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .into(),
            selected: row.selected,
            active: active.as_deref() == Some(row.id.as_str()),
            series: series.clone().into(),
            header: false,
            level: if series.is_empty() { 0 } else { 1 },
        });
    }
    ui.set_schema_rows(Rc::new(VecModel::from(model)).into());
    ui.set_schema_status(format!("{} 个可用方案 · 已勾选 {} 个", rows.len(), selected).into());
}

// ------------------------------------------------------- 界面与方案（官方对话框）

/// 主界面的「界面风格…」「方案选单…」两个按钮。
///
/// 托盘菜单里原来各有一项，用户要求收进设置（「这不是设置的部分吗」）。两条都是
/// 官方模板的 Win32 对话框，见 [`open_official`]。
fn wire_official_dialogs(ui: &SettingsWindow) {
    {
        let weak = ui.as_weak();
        ui.on_open_switcher(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(error) = open_official("/switcher") {
                    ui.set_status(format!("无法打开方案选单：{error}").into());
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_open_style(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(error) = open_official("/style") {
                    ui.set_status(format!("无法打开界面风格：{error}").into());
                }
            }
        });
    }
}

/// 另起一个自身进程，带上 `/style` 或 `/switcher` 打开官方模板对话框。
///
/// 这两个对话框都是 `DialogBoxParamW` 的 Win32 模态模板，需要由本线程的消息循环驱动；
/// 主界面已经跑在 Slint 事件循环里，不能在同一个线程里再嵌一个模态对话框，
/// 所以只能再启动一个自身进程（与官方"一个 exe 多个开关"的做法一致）。
///
/// ️ 两个进程都会写 `weasel.custom.toml`：对话框里保存后，主界面里未保存的草稿会
/// 被它判为"文件已被其他程序修改"。这是有意的保守行为，不要改成静默覆盖。
fn open_official(flag: &str) -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    std::process::Command::new(executable)
        .arg(flag)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// 打开「输入统计」窗口。`/stat` 走这里。
///
/// 数据来自 `%APPDATA%\Rime-Next\stat\stat.bin`（TIP 采集，见 `Tip/src/stats.rs`；
/// 旧的 `stats\*.toml` 只读兜底）。读文件/算数字都在 `weasel_settings::stats` 里，
/// 这里只做「数据 → Slint 属性」的搬运。
/// 把「0.1 键/秒」的整数格式化成一位小数：`52` → `"5.2"`。
///
/// 击键速度用秒速（用户 2026-10-25：分速不好算），整数太粗，所以存十分之一。
fn format_key_speed(x10: u32) -> String {
    format!("{}.{}", x10 / 10, x10 % 10)
}

/// 从码长直方图 + 总量算两个**累计**指标（尾部扩展不按天分，所以这是灰色历史值的来源）：
/// 键准 = 有效按键 ÷ 总击键（%），平均词长 = 总字数 ÷ 上屏轮数（字/轮）。
///
/// 用户 2026-10-26：这两张卡的**主值**改成"今日值"（读 v3 天记录，见
/// `snapshot.today_key_accuracy_x10` / `today_word_len_x100`），这里的累计值只作灰字的
/// **回落**口径（近 7 天算不出时用它）—— 和其它卡（今日 + 近 7 天）保持一致。
fn hist_metrics(
    key_counts: &[u32],
    code_lens: &[u32],
    total_keys: u64,
    total_chars: u64,
) -> (String, String) {
    let commits: u64 = code_lens.iter().map(|count| u64::from(*count)).sum();
    // 冰雪插件的口径（rime-snow-pinyin/docs/statistics.md）：
    // 键准 = 有效按键 / 按键，而有效按键 = 按键数 − **两倍**退格数
    // （每个键带编辑量：普通键 +1、退格 −1；Σ编辑量 = Σnj − 2×退格）。
    // 用累计 VK 计数里的退格（VK_BACK = 0x08）近似 Σ退格。
    let backspace = u64::from(*key_counts.get(8).unwrap_or(&0));
    let accuracy = if total_keys > 0 {
        let effective = total_keys as i64 - 2 * backspace as i64;
        format!("{:.1}", (effective.max(0) as f64) * 100.0 / total_keys as f64)
    } else {
        "0.0".to_string()
    };
    let word_len = if commits > 0 {
        format!("{:.2}", total_chars as f64 / commits as f64)
    } else {
        "0.00".to_string()
    };
    (accuracy, word_len)
}

/// `stat.bin` 的（修改时间, 大小）。
///
/// 自动刷新拿它判断「文件变没变」：没变就什么都不做 —— 不能每一轮（[`STAT_REFRESH_POLL`]）
/// 无条件重算，
/// 那会把 371 格热力图和一堆卡片反复重建（用户 2026-10-26：「能不能做成自动刷新」）。
fn stat_stamp() -> Option<(std::time::SystemTime, u64)> {
    let path = weasel_settings::stats::stat_file()?;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// 有效输入时长（毫秒）→ 分钟（四舍五入，最少 0）：`731876` → `"12"`。
fn format_minutes(ms: u64) -> String {
    format!("{}", (ms + 30_000) / 60_000)
}

/// 「平均每日字数」卡的灰色历史值：近 7 天平均（没有 7 天数据就除已有天数）。
fn avg_daily_note(snapshot: &weasel_settings::stats::Snapshot) -> String {
    if snapshot.recent_days == 0 {
        return "0".to_owned();
    }
    (snapshot.recent_chars / u64::from(snapshot.recent_days)).to_string()
}

/// 今日词长的显示文本。
///
/// 混合日（v2→v3 升级当天）的当日词长算不出来：`chars` 含升级前的字数、`commits` 只有
/// 升级后的轮数（2026-10-26 实测 923 ÷ 5 = 184.60）。这时回落到**累计直方图口径**
/// （用户 2026-10-26：「平均词长怎么没有数据」—— 给一个口径正确的累计值好过显示 `—`），
/// 升级次日起自动回到当日值。
fn word_len_display(snapshot: &weasel_settings::stats::Snapshot, cumulative: &str) -> String {
    if snapshot.today_word_len_mixed {
        cumulative.to_owned()
    } else {
        format_two_decimals(snapshot.today_word_len_x100)
    }
}

/// 把「0.01」单位的整数格式化成两位小数：`420` → `"4.20"`（码长、词长共用）。
fn format_two_decimals(x100: u32) -> String {
    format!("{}.{:02}", x100 / 100, x100 % 100)
}

/// 键准（单位 **0.1 %** 的整数）→ 一位小数：`987` → `"98.7"`，`1000` → `"100.0"`。
fn format_percent(x10: u32) -> String {
    format!("{}.{}", x10 / 10, x10 % 10)
}

/// 常用字词云：字号 = 14 + 30·√(频次 / 最高频次)（44px 顶格、14px 垫底），
/// 按名次切三行：最大的 3 个 / 接下来 7 个 / 其余（最多 20 个，见 `stats::MAX_WORDS`）。
///
/// 用户 2026-10-30：「常用字做成词云」——原来 20 个等大方块看不出「哪个最常用」，
/// 现在一眼就是大小差别；次数只留给最大的三个（词云里再标 20 个数字就成表格了）。

/// 词云：最常用的字在**正中**，其余按频率降序沿「黄金角向日葵螺旋」向外散开。
///
/// - 第 0 个字（出现次数最高）固定钉在画布正中心，字号最大最显眼；
/// - 其余字按黄金角螺旋（每步转 137.5°）从中心向外找空位，带 AABB 碰撞检测，
///   碰到已落位的字就继续往外螺旋，直到不重叠且落在画布内；
/// - 水平方向轻微拉抜（SX）让宽画布（1084×230）用得更饱满，竖直方向不压缩，
///   整体是一个以中心为核、向外均匀发散的「云」，**不套环、不连线、不重叠**。
/// 坐标原点是画布左上角（Slint 侧绝对定位），落点按字中心算、再转左上角。
fn cloud_layout(words: &[(String, u64)]) -> Vec<CloudWord> {
    const CANVAS_W: f32 = 1064.0;
    const CANVAS_H: f32 = 230.0;
    const CX: f32 = CANVAS_W / 2.0;
    const CY: f32 = CANVAS_H / 2.0;
    const SX: f32 = 2.8; // 水平轻微拉伸，让宽画布用得更饱满（非同心环）
    const SY: f32 = 1.0;
    const GOLDEN: f32 = 2.39996323; // 黄金角（弧度）
    const DR: f32 = 2.0; // 每步螺旋外扩半径
    const DA: f32 = 0.35; // 每步角增量（让螺旋平滑转向）
    const MAX_R: f32 = 108.0; // 半径上限：保证竖直方向不出 230 高

    struct Box {
        l: f32,
        r: f32,
        t: f32,
        b: f32,
    }
    let mut out = Vec::new();
    let mut boxes: Vec<Box> = Vec::new();

    let Some((_, max)) = words.first() else {
        return out;
    };
    let max = (*max).max(1) as f64;

    let size_of = |index: usize, count: u64| -> f32 {
        if index == 0 {
            44.0
        } else {
            let ratio = (count as f64 / max).sqrt() as f32;
            (14.0f32 + 26.0f32 * ratio).round()
        }
    };

    for (index, (ch, count)) in words.iter().take(90).enumerate() {
        let s = size_of(index, *count);
        let tint = (index % 4) as i32;

        // 第 0 个：正中固定
        let (cxw, cyw) = if index == 0 {
            (CX, CY)
        } else {
            // 从中心向外螺旋搜索无碰撞落点
            let mut r = (s / 2.0 + 4.0).max(6.0);
            let mut a = index as f32 * GOLDEN;
            let mut found = None;
            for _ in 0..400 {
                let x = CX + (r * a.cos()) * SX;
                let y = CY + (r * a.sin()) * SY;
                let l = x - s / 2.0;
                let rr = x + s / 2.0;
                let t = y - s / 2.0;
                let b = y + s / 2.0;
                let in_bounds = l >= 0.0 && rr <= CANVAS_W && t >= 0.0 && b <= CANVAS_H;
                let hit = boxes
                    .iter()
                    .any(|p| l < p.r && rr > p.l && t < p.b && b > p.t);
                if in_bounds && !hit {
                    found = Some((x, y));
                    break;
                }
                r += DR;
                a += DA;
                if r > MAX_R {
                    break;
                }
            }
            match found {
                Some(p) => p,
                // 实在放不下就放弃这个字（长尾小字极少触发，避免重叠）
                None => continue,
            }
        };

        let l = cxw - s / 2.0;
        let t = cyw - s / 2.0;
        out.push(CloudWord {
            word: ch.as_str().into(),
            size: s as i32,
            x: l.round() as i32,
            y: t.round() as i32,
            tint,
        });
        boxes.push(Box {
            l,
            r: l + s,
            t,
            b: t + s,
        });
    }
    out
}

fn split_words(words: &[(String, u64)]) -> (Vec<StatWord>, Vec<StatWord>, Vec<StatWord>) {
    let peak = words
        .first()
        .map(|(_, count)| *count as f64)
        .unwrap_or(1.0)
        .max(1.0);
    let mut big = Vec::new();
    let mut mid = Vec::new();
    let mut small = Vec::new();
    for (index, (word, count)) in words.iter().enumerate() {
        let ratio = (*count as f64 / peak).sqrt() as f32;
        let item = StatWord {
            word: word.as_str().into(),
            count: (*count).min(i32::MAX as u64) as i32,
            size: 14.0 + 30.0 * ratio,
        };
        match index {
            0..=2 => big.push(item),
            3..=9 => mid.push(item),
            _ => small.push(item),
        }
    }
    (big, mid, small)
}

/// 把采集数据灌进设置主界面的「主页」（内嵌的 `StatsPage`）。
///
/// 和 [`show_stats`] 用**同一套** `weasel_settings::stats::load()` + `snapshot()`，
/// 只是属性名多了 `stat-` 前缀 —— `SettingsWindow` 上那 15 个属性是转发给内嵌
/// `StatsPage` 的副本，所以必须两边都灌。
///
/// 返回这次算出来的 [`weasel_settings::stats::Snapshot`]：刷新按钮拿它和上一次的比，
/// 好告诉用户「数据已更新」还是「数据无变化」。
fn apply_stats_to_settings(window: &SettingsWindow) -> weasel_settings::stats::Snapshot {
    let (entries, warnings) = weasel_settings::stats::load();
    let snapshot =
        weasel_settings::stats::snapshot(&entries, weasel_settings::stats::today_local());

    let (title, level, exp_cur, exp_max) = career_level(snapshot.total_exp);
    window.set_stat_level_title(title.into());
    window.set_stat_level(level as i32);
    window.set_stat_exp_cur(exp_cur as i32);
    window.set_stat_exp_max(exp_max as i32);
    window.set_stat_exp_ratio(if exp_max > 0 {
        exp_cur as f32 / exp_max as f32
    } else {
        0.0
    });
    window.set_stat_exp_next(
        format!("还差 {} 升下一级", exp_max.saturating_sub(exp_cur)).into(),
    );
    window.set_stat_realm(career_realm(snapshot.total_chars).into());
    // 名著字数水平（趣味对照，和段位一样按累计上屏字数）
    window.set_stat_book_note(book_level(snapshot.total_chars).into());

    window.set_stat_quarter_label(snapshot.quarter_label.clone().into());
    // 今日格坐标（-1 = 不画），界面据此画十字渐变高亮。
    window.set_stat_today_week(snapshot.today_week);
    window.set_stat_today_day(snapshot.today_day);
    window.set_stat_today_keys(snapshot.today_keys.min(i32::MAX as u64) as i32);
    window.set_stat_total_keys(snapshot.total_keys.min(i32::MAX as u64) as i32);
    window.set_stat_today_chars(snapshot.today_chars.min(i32::MAX as u64) as i32);
    window.set_stat_total_chars(snapshot.total_chars.min(i32::MAX as u64) as i32);
    window.set_stat_today_speed(snapshot.today_speed as i32);
    window.set_stat_today_key_speed_text(format_key_speed(snapshot.today_key_speed_x10).into());
    window.set_stat_best_speed(snapshot.best_speed as i32);
    // 历史值写在「平均速度 / 最大速度」两张卡的值下面（用户 2026-10-25 要求加括号）。
    // 「平均」类灰字 = **近 7 天**（用户 2026-10-26 定口径：数据全量记，平均只看 7 天）。
    window.set_stat_all_speed_note(format!("{}", snapshot.recent_speed).into());
    window.set_stat_best_speed_note(format!("{}", snapshot.best_speed).into());
    window.set_stat_best_key_speed_text(format_key_speed(snapshot.best_key_speed_x10).into());
    window.set_stat_all_key_speed_note(format_key_speed(snapshot.recent_key_speed_x10).into());
    window.set_stat_best_key_speed_note(format_key_speed(snapshot.best_key_speed_x10).into());
    window.set_stat_total_chars_note(format!("{}", snapshot.total_chars).into());
    window.set_stat_total_keys_note(format!("{}", snapshot.total_keys).into());
    window.set_stat_code_len_text(format_two_decimals(snapshot.today_code_len_x100).into());
    window.set_stat_all_code_len_note(format_two_decimals(snapshot.recent_code_len_x100).into());
    window.set_stat_best_streak_note(format!("{}", snapshot.best_streak_days).into());
    window.set_stat_streak_days(snapshot.streak_days as i32);

    let day_cells: Vec<StatDay> = snapshot
        .days
        .iter()
        .map(|cell| StatDay {
            count: cell.count as i32,
            tip: cell.tip.as_str().into(),
            today: cell.today,
        })
        .collect();
    window.set_stat_days(slint::ModelRc::new(slint::VecModel::from(day_cells)));

    // 年度活跃记录（12 月 × 7 天）：聚合在 `stats.rs` 里做（DayCell 没有月份字段）。
    let month_cells: Vec<StatDay> = snapshot
        .month_days
        .iter()
        .map(|cell| StatDay {
            count: cell.count as i32,
            tip: cell.tip.as_str().into(),
            today: cell.today,
        })
        .collect();
    window.set_stat_month_days(slint::ModelRc::new(slint::VecModel::from(month_cells)));
    window.set_stat_active_days(snapshot.active_days as i32);
    window.set_stat_active_rate(snapshot.active_rate as i32);
    let cal_cells: Vec<StatDay> = snapshot
        .calendar
        .iter()
        .map(|cell| StatDay {
            count: cell.count as i32,
            tip: cell.tip.as_str().into(),
            today: cell.today,
        })
        .collect();
    window.set_stat_calendar(slint::ModelRc::new(slint::VecModel::from(cal_cells)));
    window.set_stat_calendar_cols(slint::ModelRc::new(slint::VecModel::from(snapshot.calendar_cols.clone())));
    window.set_stat_calendar_offsets(slint::ModelRc::new(slint::VecModel::from(snapshot.calendar_offsets.clone())));
    window.set_stat_calendar_mask(slint::ModelRc::new(slint::VecModel::from(snapshot.calendar_mask.clone())));
    window.set_stat_calendar_cols_total(snapshot.calendar_cols_total);
    window.set_stat_month_week(slint::ModelRc::new(slint::VecModel::from(
        snapshot.month_week.iter().map(|w| *w).collect::<Vec<i32>>(),
    )));

    // 常用字排两行（用户 2026-10-25：做成两行）：前 10 个一行，其余一行。
    let (words_big, words_mid, words_small) = split_words(&snapshot.words);
    window.set_stat_words(slint::ModelRc::new(slint::VecModel::from(words_big)));
    // 词云：排版在 Rust 侧算（字号 + 坐标），Slint 只按坐标绝对定位画
    window.set_stat_cloud(slint::ModelRc::new(slint::VecModel::from(cloud_layout(&snapshot.words))));
    window.set_stat_words2(slint::ModelRc::new(slint::VecModel::from(words_mid)));
    window.set_stat_words3(slint::ModelRc::new(slint::VecModel::from(words_small)));

    // 键盘热力图 / 码长直方图（stat.bin 的尾部扩展；老文件没有就传空）。
    let (key_counts, code_lens) = weasel_settings::stats::load_hist();
    // 时段统计：24 个桶归一化成 0..100 的百分比（Slint 侧直接当高度用）。
    // 演示/自测：`WEASEL_HOUR_SAMPLE=1` 时用一条假的小时曲线，**不读也不写真实 stat.bin**。
    let (hour_keys, hour_ms) = if std::env::var_os("WEASEL_HOUR_SAMPLE").is_some() {
        let curve = [0u32, 0, 0, 0, 0, 0, 0, 12, 340, 620, 480, 390, 410, 300, 260, 330, 470, 610, 720, 810, 640, 380, 120, 30];
        let mut ms = [0u64; 24];
        for (index, value) in curve.iter().enumerate() {
            ms[index] = u64::from(*value) * 120;
        }
        (curve.to_vec(), ms.to_vec())
    } else {
        let (keys, ms) = weasel_settings::stats::load_hours();
        (keys.to_vec(), ms.to_vec())
    };
    let hour_peak_idx = hour_keys.iter().enumerate().max_by_key(|(_, v)| **v).map(|(i, _)| i as i32).unwrap_or(0);
    let hour_total_min = hour_ms.iter().sum::<u64>() / 60_000;
    let hour_summary = format!("最活跃 {hour_peak_idx} 时 · 共 {hour_total_min} 分（按有效输入时长）");
    let hour_peak = hour_keys.iter().copied().max().unwrap_or(0).max(1) as f32;
    let hour_pct: Vec<i32> = hour_keys
        .iter()
        .map(|value| ((*value as f32 / hour_peak) * 100.0).round() as i32)
        .collect();
    window.set_stat_hour_keys(Rc::new(VecModel::from(hour_pct)).into());
    window.set_stat_hour_peak(hour_peak_idx);
    window.set_stat_hour_summary(hour_summary.as_str().into());
    // 键准/词长：能用近 7 天（v3 天记录）就用近 7 天，否则回落到全期直方图口径
    // （stat.bin 还是 v2 时算不出近 7 天键准/词长，不能让灰字显示 0）。
    let (hist_acc, hist_wlen) =
        hist_metrics(&key_counts, &code_lens, snapshot.total_keys, snapshot.total_chars);
    let acc = if snapshot.recent_key_accuracy_x10 > 0 {
        format_percent(snapshot.recent_key_accuracy_x10)
    } else {
        hist_acc
    };
    let wlen = if snapshot.recent_word_len_x100 > 0 {
        format_two_decimals(snapshot.recent_word_len_x100)
    } else {
        hist_wlen.clone()
    };
    // 键准 / 平均词长（用户 2026-10-26）：主值 = **今日**（v3 天记录的当日数），
    // 灰字 = **近 7 天**（`wlen`：v3 天算不出时回落累计直方图口径）—— 和别的卡一致。
    window.set_stat_key_accuracy_text(format_percent(snapshot.today_key_accuracy_x10).into());
    window.set_stat_word_len_text(word_len_display(&snapshot, &hist_wlen).into());
    window.set_stat_key_accuracy_note(acc.into());
    window.set_stat_word_len_note(wlen.into());
    // 使用时间：今日 + 累计（都用分钟；`active_ms` 是「按键间隔 ≤5 秒」的有效输入时长）
    window.set_stat_time_today_text(format_minutes(snapshot.today_active_ms).into());
    window.set_stat_time_total_note(format_minutes(snapshot.total_active_ms).into());
    // 平均每日字数：全期（只除有击键的天）+ 近 7 天（灰字）
    window.set_stat_avg_daily_text(snapshot.avg_daily_chars.to_string().into());
    window.set_stat_avg_daily_note(avg_daily_note(&snapshot).into());
    let (finger_hands, finger_rows, finger_names) =
        weasel_settings::stats::finger_breakdown(&key_counts);
    let peak = key_counts.iter().copied().max().unwrap_or(0) as i32;
    window.set_stat_key_counts(slint::ModelRc::new(slint::VecModel::from(
        key_counts
            .iter()
            .map(|count| *count as i32)
            .collect::<Vec<i32>>(),
    )));
        window.set_stat_finger_hands(finger_hands.clone().into());
        window.set_stat_finger_rows(finger_rows.clone().into());
        window.set_stat_finger_names(finger_names.clone().into());
    window.set_stat_code_lens(slint::ModelRc::new(slint::VecModel::from(
        code_lens.iter().map(|count| *count as i32).collect::<Vec<i32>>(),
    )));
    window.set_stat_key_peak(peak.max(1));

    window.set_stat_status(
        if let Some(first) = warnings.first() {
            format!("统计文件 {first} 读取有问题").into()
        } else if snapshot.has_data {
            "".into()
        } else {
            "还没有采集到数据：替换掉 TIP（Weasel-Next.dll）后打的字才会开始累计。".into()
        },
    );
    snapshot
}

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
        ui.set_book_note(book_level(snapshot.total_chars).into());

        ui.set_quarter_label(snapshot.quarter_label.clone().into());
        ui.set_today_week(snapshot.today_week);
        ui.set_today_day(snapshot.today_day);
        ui.set_today_keys(snapshot.today_keys.min(i32::MAX as u64) as i32);
        ui.set_total_keys(snapshot.total_keys.min(i32::MAX as u64) as i32);
        ui.set_today_chars(snapshot.today_chars.min(i32::MAX as u64) as i32);
        ui.set_total_chars(snapshot.total_chars.min(i32::MAX as u64) as i32);
        ui.set_today_speed(snapshot.today_speed as i32);
        ui.set_today_key_speed_text(format_key_speed(snapshot.today_key_speed_x10).into());
        ui.set_best_speed(snapshot.best_speed as i32);
        ui.set_all_speed_note(format!("{}", snapshot.recent_speed).into());
        ui.set_best_speed_note(format!("{}", snapshot.best_speed).into());
        ui.set_best_key_speed_text(format_key_speed(snapshot.best_key_speed_x10).into());
        ui.set_all_key_speed_note(format_key_speed(snapshot.recent_key_speed_x10).into());
        ui.set_best_key_speed_note(format_key_speed(snapshot.best_key_speed_x10).into());
        ui.set_total_chars_note(format!("{}", snapshot.total_chars).into());
        ui.set_total_keys_note(format!("{}", snapshot.total_keys).into());
        ui.set_code_len_text(format_two_decimals(snapshot.today_code_len_x100).into());
        ui.set_all_code_len_note(format_two_decimals(snapshot.recent_code_len_x100).into());
        ui.set_best_streak_note(format!("{}", snapshot.best_streak_days).into());
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

        let (words_big, words_mid, words_small) = split_words(&snapshot.words);
        ui.set_words(slint::ModelRc::new(slint::VecModel::from(words_big)));
        ui.set_words2(slint::ModelRc::new(slint::VecModel::from(words_mid)));
        ui.set_words3(slint::ModelRc::new(slint::VecModel::from(words_small)));

        let (key_counts, code_lens) = weasel_settings::stats::load_hist();
    // 时段统计：24 个桶归一化成 0..100 的百分比（Slint 侧直接当高度用）。
    // 演示/自测：`WEASEL_HOUR_SAMPLE=1` 时用一条假的小时曲线，**不读也不写真实 stat.bin**。
    let (hour_keys, hour_ms) = if std::env::var_os("WEASEL_HOUR_SAMPLE").is_some() {
        let curve = [0u32, 0, 0, 0, 0, 0, 0, 12, 340, 620, 480, 390, 410, 300, 260, 330, 470, 610, 720, 810, 640, 380, 120, 30];
        let mut ms = [0u64; 24];
        for (index, value) in curve.iter().enumerate() {
            ms[index] = u64::from(*value) * 120;
        }
        (curve.to_vec(), ms.to_vec())
    } else {
        let (keys, ms) = weasel_settings::stats::load_hours();
        (keys.to_vec(), ms.to_vec())
    };
    let hour_peak_idx = hour_keys.iter().enumerate().max_by_key(|(_, v)| **v).map(|(i, _)| i as i32).unwrap_or(0);
    let hour_total_min = hour_ms.iter().sum::<u64>() / 60_000;
    let hour_summary = format!("最活跃 {hour_peak_idx} 时 · 共 {hour_total_min} 分（按有效输入时长）");
    let hour_peak = hour_keys.iter().copied().max().unwrap_or(0).max(1) as f32;
    let hour_pct: Vec<i32> = hour_keys
        .iter()
        .map(|value| ((*value as f32 / hour_peak) * 100.0).round() as i32)
        .collect();
    ui.set_hour_keys(Rc::new(VecModel::from(hour_pct)).into());
    ui.set_hour_peak(hour_peak_idx);
    ui.set_hour_summary(hour_summary.as_str().into());
        // 键准/词长：能用近 7 天（v3 天记录）就用近 7 天，否则回落到全期直方图口径
        // （stat.bin 还是 v2 时算不出近 7 天键准/词长，不能让灰字显示 0）。
        let (hist_acc, hist_wlen) =
            hist_metrics(&key_counts, &code_lens, snapshot.total_keys, snapshot.total_chars);
        let acc = if snapshot.recent_key_accuracy_x10 > 0 {
            format_percent(snapshot.recent_key_accuracy_x10)
        } else {
            hist_acc
        };
        let wlen = if snapshot.recent_word_len_x100 > 0 {
            format_two_decimals(snapshot.recent_word_len_x100)
        } else {
            hist_wlen.clone()
        };
        // 主值 = 今日（v3 天记录），灰字 = 近 7 天（算不出回落累计，和主页那套完全一致）。
        ui.set_key_accuracy_text(format_percent(snapshot.today_key_accuracy_x10).into());
        ui.set_word_len_text(word_len_display(&snapshot, &hist_wlen).into());
        ui.set_key_accuracy_note(acc.into());
        ui.set_word_len_note(wlen.into());
        ui.set_time_today_text(format_minutes(snapshot.today_active_ms).into());
        ui.set_time_total_note(format_minutes(snapshot.total_active_ms).into());
        ui.set_avg_daily_text(snapshot.avg_daily_chars.to_string().into());
        ui.set_avg_daily_note(avg_daily_note(&snapshot).into());
        let (finger_hands, finger_rows, finger_names) =
            weasel_settings::stats::finger_breakdown(&key_counts);
        let peak = key_counts.iter().copied().max().unwrap_or(0) as i32;
        ui.set_key_counts(slint::ModelRc::new(slint::VecModel::from(
            key_counts
                .iter()
                .map(|count| *count as i32)
                .collect::<Vec<i32>>(),
        )));
        ui.set_finger_hands(finger_hands.clone().into());
        ui.set_finger_rows(finger_rows.clone().into());
        ui.set_finger_names(finger_names.clone().into());
        ui.set_code_lens(slint::ModelRc::new(slint::VecModel::from(
            code_lens.iter().map(|count| *count as i32).collect::<Vec<i32>>(),
        )));
        ui.set_key_peak(peak.max(1));

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
    // 自动刷新：和主页同一套判断（文件变了才重算）。
    let auto = slint::Timer::default();
    {
        let apply = apply.clone();
        let stamp = Rc::new(RefCell::new(stat_stamp()));
        auto.start(
            slint::TimerMode::Repeated,
            STAT_REFRESH_POLL,
            move || {
                let now = stat_stamp();
                if now.is_none() || *stamp.borrow() == now {
                    return;
                }
                *stamp.borrow_mut() = now;
                apply();
            },
        );
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
    // ⚠️ 单位：`window.size()` 在**本项目锁定的 Slint 版本里返回的是物理像素**
    // （实测 125% 缩放、preferred 1280×682 时它给 1600×853 = 1280×682×1.25），
    // 正好和 `SPI_GETWORKAREA` 的物理工作区同一口径 ⇒ **不要再乘 scale_factor()**，
    // 乘了会变成 2000×1114、直接顶到工作区左上角（2026-10-26 实测 pos=(78,7)）。
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
    let path = user_data_dir().join("user.yaml");
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
/// 100 级段位曲线：`req(n) = 50(n-1) + 50(n-1)²`。
///
/// 前 5 级正好是 100 / 300 / 600 / 1000（与旧的手写表一致），
/// 上级到 100 级约 49.5 万经验（≈ 上屏 49.5 万字），普通人几年可满级。
/// 旧表用 `r*2+100` 会指数爆炸（20 级就到 5200 万），100 级根本到不了。
const CAREER_MAX_LEVEL: u32 = 100;

fn career_level_req(level: u32) -> u64 {
    if level <= 1 {
        return 0;
    }
    let n = u64::from(level - 1);
    50 * n + 50 * n * n
}

/// 每 10 级一个段位称号（等级越高，称号越"仙"）。
const CAREER_TITLES: &[&str] = &[
    "初识键盘",
    "摸键学徒",
    "码字新人",
    "指尖练习生",
    "输入学徒",
    "码流行者",
    "飞键使",
    "连击者",
    "码阵构筑师",
    "熟练输入师",
];

fn career_title(level: u32) -> &'static str {
    let index = ((level.saturating_sub(1)) / 10) as usize;
    CAREER_TITLES[index.min(CAREER_TITLES.len() - 1)]
}

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
    // 二分出等级：最大的 n 使 req(n) <= total_exp
    let mut lo = 1u32;
    let mut hi = CAREER_MAX_LEVEL;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if career_level_req(mid) <= total_exp {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let level = lo;
    let start = career_level_req(level);
    let end = career_level_req((level + 1).min(CAREER_MAX_LEVEL + 1));
    let span = if end > start { end - start } else { 1 };
    (career_title(level), level, total_exp.saturating_sub(start), span)
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

// ── 名著字数水平（趣味对照：累计上屏字数 ≈ 某部名著的篇幅）──────────────
//
// 字数是各书**通行版本的约数**（不同版本、不同统计口径会有出入），只用来给打字量
// 一个直观参照，不追求精确。表按字数**升序**排列，`book_level` 报"还差多少字到下一部"。


const BOOK_LEVELS: &[(u64, &str)] = &[
    (5_000, "《道德经》"),
    (6_000, "《孙子兵法》"),
    (16_000, "《论语》"),
    (35_000, "《孟子》"),
    (80_000, "《庄子》"),
    (196_000, "《左传》"),
    (360_000, "《三国志》"),
    (526_000, "《史记》"),
    (640_000, "《三国演义》"),
    (731_000, "《红楼梦》"),
    (820_000, "《西游记》"),
    (960_000, "《水浒传》"),
    (3_000_000, "《资治通鉴》"),
    (40_000_000, "《二十四史》"),
];

/// 字数格式化：满一万写成"X.X 万字"，否则直接写"X 字"。
fn format_chars(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1} 万字", n as f64 / 10_000.0)
    } else {
        format!("{n} 字")
    }
}

/// 展示时最多列几个"下一个里程碑"（一行放不下更多）。
///
/// 用户 2026-10-29 定的口径：**只报"还差多少字到哪一部"**，
/// 「已写多少 ≈ A+B」那种加法折算与"再写多少字加上 C"的说法都不要。
const BOOK_UPCOMING_SHOWN: usize = 3;

/// 累计字数还没超过的下一批名著（按字数升序，最多 [`BOOK_UPCOMING_SHOWN`] 部）。
///
/// 返回 `(目标字数, 书名)`；目标只随字数前进，不会因为别的书被"折算掉"而回头。
fn upcoming_books(total_chars: u64) -> Vec<(u64, &'static str)> {
    BOOK_LEVELS
        .iter()
        .filter(|(base, _)| *base > total_chars)
        .take(BOOK_UPCOMING_SHOWN)
        .map(|(base, name)| (*base, *name))
        .collect()
}

/// 名著字数水平：已写多少字 + **还差多少字到 a / 到 b / 到 c**。
///
/// 例（6 万字）：`已写 6.0 万字；还差 2.0 万字到《庄子》、13.6 万字到《左传》、30.0 万字到《三国志》`
fn book_level(total_chars: u64) -> String {
    let upcoming = upcoming_books(total_chars);
    let mut text = format!("已写 {}", format_chars(total_chars));
    if upcoming.is_empty() {
        text.push_str("，已经超过《二十四史》的篇幅");
        return text;
    }
    for (index, (base, name)) in upcoming.iter().enumerate() {
        text.push_str(if index == 0 { "；还差 " } else { "、" });
        text.push_str(&format!("{}到{}", format_chars(base - total_chars), name));
    }
    text
}

// ── 词库页（tab 5）──────────────────────────────────────────────────────
fn wire_dicts(ui: &SettingsWindow) {
    let user = user_data_dir();

    let refresh: std::rc::Rc<dyn Fn()> = {
        let user = user.clone();
        let ui_weak = ui.as_weak();
        std::rc::Rc::new(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let dicts = weasel_settings::dict::list_dicts(&user).unwrap_or_default();
            let code = ui.get_dict_code().to_string();
            let mut rows: Vec<DictRow> = Vec::new();
            if !code.trim().is_empty() {
                for dict in &dicts {
                    let Ok(text) = std::fs::read_to_string(&dict.path) else { continue };
                    for (rank, entry) in weasel_settings::dict::find_entries(&text, &code, true) {
                        rows.push(DictRow {
                            rank: rank as i32,
                            word: entry.word.as_str().into(),
                            code: entry.code.as_str().into(),
                            source: dict.name.as_str().into(),
                        });
                    }
                }
            }
            let total: usize = dicts.iter().map(|dict| dict.entries).sum();
            ui.set_dict_rows(std::rc::Rc::new(slint::VecModel::from(rows)).into());
            ui.set_dict_status(format!("{} 个词典 · 共 {} 条", dicts.len(), total).into());
        })
    };

    {
        let refresh = std::rc::Rc::clone(&refresh);
        ui.on_dict_refresh(move || refresh());
    }
    {
        let refresh = std::rc::Rc::clone(&refresh);
        ui.on_dict_find(move || refresh());
    }
    {
        let user = user.clone();
        let ui_weak = ui.as_weak();
        let refresh = std::rc::Rc::clone(&refresh);
        ui.on_dict_add(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let code = ui.get_dict_code().to_string();
            let word = ui.get_dict_word().to_string();
            let Ok(dicts) = weasel_settings::dict::list_dicts(&user) else { return };
            let Some(dict) = dicts.first() else {
                ui.set_dict_status("用户目录里没有可写的 *.dict.yaml".into());
                return;
            };
            let Ok(text) = std::fs::read_to_string(&dict.path) else { return };
            match weasel_settings::dict::add_entry(&text, &word, &code) {
                Ok((content, note)) => {
                    // 写前先备份（学自蓝宝石的 backupNow）
                    let _ = weasel_settings::dict::backup_dict(&user, &dict.path);
                    if std::fs::write(&dict.path, content).is_ok() {
                        ui.set_dict_status(format!("{note}（已备份）").into());
                    }
                }
                Err(error) => ui.set_dict_status(error.into()),
            }
            refresh();
        });
    }
    {
        let user = user.clone();
        let ui_weak = ui.as_weak();
        let refresh = std::rc::Rc::clone(&refresh);
        ui.on_dict_remove(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let word = ui.get_dict_word().to_string();
            let Ok(dicts) = weasel_settings::dict::list_dicts(&user) else { return };
            let mut removed = 0usize;
            for dict in &dicts {
                let Ok(text) = std::fs::read_to_string(&dict.path) else { continue };
                let (content, count) = weasel_settings::dict::remove_entry(&text, &word);
                if count == 0 { continue; }
                let _ = weasel_settings::dict::backup_dict(&user, &dict.path);
                if std::fs::write(&dict.path, content).is_ok() { removed += count; }
            }
            ui.set_dict_status(
                if removed > 0 {
                    format!("已删除 {removed} 条「{word}」（已备份）")
                } else {
                    format!("没有找到词条「{word}」")
                }
                .into(),
            );
            refresh();
        });
    }
}

/// 下载页：浏览 GitHub Rime-Next 组织仓库 + 一键拉取搬运。
///
/// 组织名、目标目录写死；镜像自动探测由 plum 内部完成，UI 不暴露任何输入。
/// 切换分类 / 安装完成 / 切回本页均自动刷新。
/// 返回 `load_list` 供切页签时触发刷新。
fn wire_downloads(ui: &SettingsWindow) -> std::sync::Arc<dyn Fn() + Send + Sync> {
    let user = user_data_dir();

    // 拉取组织仓库列表（后台线程，经 upgrade_in_event_loop 回传结果）
    // `user` 后面安装逻辑还要用，所以给列表闭包一份克隆，别把它 move 走。
    let user_for_list = user.clone();
    let load_list: std::sync::Arc<dyn Fn() + Send + Sync> = {
        let ui_weak = ui.as_weak();
        std::sync::Arc::new(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            ui.set_download_list_status("正在从 GitHub 加载...".into());
            let ui_weak2 = ui_weak.clone();
            let user_root_f = user_for_list.clone();
            std::thread::spawn(move || {
                match plum_next::org::list_repos(plum_next::org::ORG) {
                    Ok(repos) => {
                        let mut rows: Vec<RepoRow> = Vec::new();
                        let mut cats: Vec<String> = vec!["全部".to_owned()];
                        for r in &repos {
                            let cat = weasel_settings::download::category_of(&r.name, &r.topics);
                            if !cats.contains(&cat) {
                                cats.push(cat.clone());
                            }
                            let initial = r
                                .name
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().to_string())
                                .unwrap_or_else(|| "?".to_owned());
                            // 已安装清单：装了哪一版、远端从那以后有没有 push。
                            let full = format!("{}/{}", plum_next::org::ORG, r.name);
                            let installed = weasel_settings::download::load_installed(&user_root_f)
                                .get(&full)
                                .cloned();
                            let badge = weasel_settings::download::badge_for(
                                installed.as_ref(),
                                r.pushed_at.as_deref(),
                            );
                            rows.push(RepoRow {
                                name: r.name.clone().into(),
                                description: r.description.clone().unwrap_or_default().into(),
                                clone_url: r.clone_url.clone().into(),
                                default_branch: r.default_branch.clone().unwrap_or_default().into(),
                                stargazers: r.stargazers_count.unwrap_or(0) as i32,
                                language: r.language.clone().unwrap_or_default().into(),
                                category: cat.clone().into(),
                                initial: initial.into(),
                                badge: badge.into(),
                                pushed_at: r.pushed_at.clone().unwrap_or_default().into(),
                            });
                        }
                        let repos_len = repos.len();
                        let _ = ui_weak2.upgrade_in_event_loop(move |ui| {
                            ui.set_download_repos(Rc::new(slint::VecModel::from(rows)).into());
                            let cats_shared: Vec<slint::SharedString> =
                                cats.iter().map(|c| c.clone().into()).collect();
                            ui.set_download_categories(Rc::new(slint::VecModel::from(cats_shared)).into());
                            ui.set_download_list_status(format!("共 {} 个仓库", repos_len).into());
                            ui.set_download_net_error("".into());
                        });
                    }
                    Err((e, network)) => {
                        let net_msg = if network { Some(e.clone()) } else { None };
                        let status = if network {
                            String::new()
                        } else {
                            format!("加载失败: {e}")
                        };
                        let _ = ui_weak2.upgrade_in_event_loop(|ui| {
                            ui.set_download_list_status(status.into());
                            if let Some(msg) = net_msg {
                                ui.set_download_net_error(msg.into());
                            }
                        });
                    }
                }
            });
        })
    };

    {
        let load_list = std::sync::Arc::clone(&load_list);
        ui.on_download_tab_selected(move |_cat| load_list());
    }

    // 安装一个仓库
    {
        let ui_weak = ui.as_weak();
        let root = user.clone();
        let load_list = std::sync::Arc::clone(&load_list);
        ui.on_download_install(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let model = ui.get_download_repos();
            let Some(repo) = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .find(|r| r.name == name)
            else { return; };
            let clone_url = repo.clone_url.clone();
            let branch = repo.default_branch.clone();
            let pushed_at = repo.pushed_at.to_string();
            let root = root.clone();

            ui.set_download_installing_name(name.clone().into());
            ui.set_download_frac(0.0.into());

            let ui_weak2 = ui_weak.clone();
            let ui_weak3 = ui_weak.clone();
            let load_list2 = std::sync::Arc::clone(&load_list);
            std::thread::spawn(move || {
                let opts = plum_next::fetch::FetchOpts { proxy: None };
                let branch_opt = if branch.is_empty() { None } else { Some(branch.as_str()) };
                let result = plum_next::relocate::pull_and_relocate_all(
                    &root, &clone_url, branch_opt, false, &opts,
                    |_stage, frac, _msg| {
                        let f = frac as f32;
                        let w = ui_weak2.clone();
                        let _ = w.upgrade_in_event_loop(move |ui| ui.set_download_frac(f));
                    },
                )
                .map(|r| r.repo_name)
                .map_err(|e| {
                    let net = weasel_settings::download::is_network_error(&e);
                    (e, net)
                });
                let root_for_record = root.clone();
                let pushed_at_for_record = pushed_at.clone();
                let _ = ui_weak3.upgrade_in_event_loop(move |ui| {
                    ui.set_download_installing_name("".into());
                    ui.set_download_frac(1.0.into());
                    match result {
                        Ok(repo_name) => {
                            // ① 记清单：装了哪一版（远端 pushed_at），下次列表据此算「可更新」
                            let full = format!("{}/{}", plum_next::org::ORG, repo_name);
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let _ = weasel_settings::download::record_installed(
                                &root_for_record,
                                weasel_settings::download::InstalledRepo {
                                    repo: full,
                                    pushed_at: pushed_at_for_record.clone(),
                                    installed_at: format!("epoch:{now}"),
                                },
                            );
                            ui.set_download_installed_name(repo_name.clone().into());
                            ui.set_download_net_error("".into());

                            // ② 装完自动部署（用户 2026-10-30：拉取要「拉取并部署」）。
                            //    broker 干活，这里只发命令 + 轮询 last_build_time 判断完成。
                            let before = read_last_build_time();
                            let _ = std::panic::catch_unwind(|| crate::switcher_dialog::request_deploy());
                            ui.set_download_list_status(
                                format!("已拉取：{repo_name}，正在部署…").into(),
                            );
                            let ui_weak4 = ui.as_weak();
                            std::thread::spawn(move || {
                                for _ in 0..120 {
                                    std::thread::sleep(std::time::Duration::from_millis(500));
                                    if read_last_build_time() != before {
                                        let _ = ui_weak4.upgrade_in_event_loop(|ui| {
                                            ui.set_download_list_status(
                                                "已完成拉取并部署，可以直接用了".into(),
                                            );
                                        });
                                        return;
                                    }
                                }
                                let _ = ui_weak4.upgrade_in_event_loop(|ui| {
                                    ui.set_download_list_status(
                                        "已拉取，但部署 60 秒内没完成（可到「更新」页手动部署）"
                                            .into(),
                                    );
                                });
                            });
                        }
                        Err((e, net)) => {
                            ui.set_download_list_status(format!("失败: {e}").into());
                            if net {
                                ui.set_download_net_error("网络波动，安装中断，请检查网络后重试".into());
                            }
                        }
                    }
                    load_list2();
                });
            });
        });
    }

    // 关闭网络通知横幅
    {
        let ui_weak = ui.as_weak();
        ui.on_download_dismiss_net(move || {
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_download_net_error("".into());
            }
        });
    }
    load_list
}

#[cfg(test)]
mod tests {
    use super::{book_level, format_chars};

    #[test]
    fn format_chars_switches_to_wan_above_ten_thousand() {
        assert_eq!(format_chars(0), "0 字");
        assert_eq!(format_chars(5_000), "5000 字");
        assert_eq!(format_chars(10_000), "1.0 万字");
        assert_eq!(format_chars(526_000), "52.6 万字");
    }

    /// 还没写到第一部时只提示还差多少，不能下溢。
    #[test]
    fn book_level_before_first_book() {
        let text = book_level(0);
        assert!(text.contains("《道德经》"), "{text}");
        assert!(text.contains("还差"), "{text}");
    }

    /// 用户 2026-10-29 的口径：**只报"还差多少字到 a / b / c"**，
    /// 不要"已写 ≈ A+B"的折算、也不要"再写多少字加上 C"。
    #[test]
    fn book_level_lists_remaining_chars_to_next_three() {
        // 6 万字：下一部是《庄子》(8 万，差 2 万)、《左传》(19.6 万，差 13.6 万)、
        // 《三国志》(36 万，差 30 万)。
        let text = book_level(60_000);
        assert!(text.starts_with("已写 6.0 万字"), "{text}");
        assert!(text.contains("还差 2.0 万字到《庄子》"), "{text}");
        assert!(text.contains("13.6 万字到《左传》"), "{text}");
        assert!(text.contains("30.0 万字到《三国志》"), "{text}");
        assert!(!text.contains('+'), "不要加法折算：{text}");
        assert!(!text.contains("再写"), "不要「再写…加上」：{text}");
        // 最多三项目标
        assert_eq!(text.matches("到《").count(), 3, "{text}");

        // 100 万字：已经超过《水浒传》，下一部是《资治通鉴》(300 万)。
        let text = book_level(1_000_000);
        assert!(text.contains("还差 200.0 万字到《资治通鉴》"), "{text}");
        assert!(text.contains("《二十四史》"), "{text}");
    }

    /// 目标只前进、不回头：字数越多，第一个"还差到"的书只能越来越靠后，
    /// 且差额恒为正（旧实现会在 5000→6000 字时从《孙子兵法》跳回《道德经》）。
    #[test]
    fn upcoming_books_only_move_forward() {
        use super::{upcoming_books, BOOK_LEVELS};
        let mut last = 0usize;
        for total in (0..=1_000_000).step_by(500) {
            let upcoming = upcoming_books(total);
            if upcoming.is_empty() {
                continue;
            }
            let (base, name) = upcoming[0];
            let index = BOOK_LEVELS.iter().position(|(_, n)| *n == name).unwrap();
            assert!(index >= last, "目标回退：{total} 字 → {name}");
            assert!(base > total, "差额必须为正：{total} 字 → {name}");
            assert!(upcoming.len() <= 3, "最多三项目标");
            last = index;
        }
    }

    /// 超过最后一部时不再找下一部，不能越界。
    #[test]
    fn book_level_beyond_last_book() {
        let text = book_level(100_000_000);
        assert!(text.contains("已经超过《二十四史》的篇幅"), "{text}");
        assert!(!text.contains("还差"), "没有下一部时不要报差额：{text}");
    }
}
