//! 配置描述仅启动时读取；主题选择只改变单页分区显隐。
#![windows_subsystem = "windows"]
mod backdrop;
mod bindings;
mod broker;
mod colors;
mod dialogs;
mod dict_dialog;
mod style_keys;
mod switcher_dialog;
mod single_instance;
mod skins;
mod frame;
mod updater;
mod wasm_modules;
// 框架无关的设置核心来自 weasel_settings 库
// （配置文档读写与冲突检测、主题目录发现、表单字段模型、应用选项编辑）。
use weasel_settings::{app_options, catalog, config, form, schema_tree};
use slint::{ComponentHandle, Model, VecModel};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashSet},
    rc::Rc,
};
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

/// 共享（出厂）数据目录：`default.yaml` 等出厂资源的回退来源，与 `wire_skins`
/// 里 `skins::resolve_shared_dir` 同一套查找（exe 旁边 → 上溯 Rime-Next → ProgramFiles）。
fn shared_data_dir() -> std::path::PathBuf {
    let script_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    let probe = script_dir.join("theme").join("default").join("default.theme.yaml");
    skins::resolve_shared_dir(&script_dir, &probe)
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
    // 不占这把锁 —— 否则设置窗开着时，托盘的「菜单 / 外观」会全都打不开。
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
    // 「设置」弹出来的是菜单设置对话框，用户 2026-10-17 21:11 报过
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
        // 导航信号槽（2026-11-14）：界面上的「方案」等标签只发 `page-selected`，
        // **不再直接赋值 current-page**。这里用 lib 的分发器把一条信号展开成
        // 「切页 → 进入页面 → 加载数据」，命令行 / 环境变量也走同一个槽。
        let weak = window.as_weak();
        let loader = std::sync::Arc::clone(&download_loader);
        window.on_page_selected(move |page| {
            use weasel_settings::signals::{
                PAGE_COUNT, PAGE_DOWNLOAD, PAGE_SETTINGS, Signal, dispatch,
            };
            let Some(window) = weak.upgrade() else {
                return;
            };
            let (target, out) = dispatch(
                Signal::PageSelected(page.max(0) as usize),
                window.get_current_page().max(0) as usize,
                PAGE_COUNT,
            );
            // 页面状态只在这里落地。
            window.set_current_page(target as i32);
            for signal in out {
                match signal {
                    Signal::PageEntered(p) | Signal::PageReselected(p) => {
                        // 重算当前页的字段模型（筛选放在 Rust 侧，界面上不留空行）。
                        refresh_page_fields(&window);
                        // 切到「常规」页时重读一次当前值：用户可能在别处改过 theme.yaml
                        // 或 default.yaml（每页候选数）。
                        if p == PAGE_SETTINGS {
                            let user = user_data_dir();
                            window.set_font_point(dialogs::current_font_point(&user) as i32);
                            let shared = shared_data_dir();
                            window.set_page_size(dialogs::current_page_size(&user, &shared) as i32);
                        }
                        if p == PAGE_DOWNLOAD {
                            loader();
                        }
                    }
                    Signal::ReloadStats => {
                        let _ = apply_stats_to_settings(&window);
                    }
                    // 进入「方案」界面 ⇒ 重新读盘：CLI / 官方对话框改过清单也不会看到旧列表。
                    Signal::ReloadSchemas => window.invoke_schema_refresh(),
                    Signal::ReloadDicts => window.invoke_dict_refresh(),
                    Signal::PageSelected(_) => {}
                }
            }
        });
    }
    // 运维/回归用：WEASEL_SETTING_PAGE=<0..6> 让无人值守启动时直接落在指定页签，
    // 方便截图核对（默认的「主页」行为完全不变；正常托盘打开仍不带这个变量）。
    if let Ok(raw) = std::env::var("WEASEL_SETTING_PAGE") {
        if let Ok(page) = raw.parse::<i32>() {
            if (0..=6).contains(&page) {
                // 只发信号：current-page 由 on_page_selected 槽统一设置，
                // 与鼠标点标签走完全相同的分发路径。
                window.invoke_page_selected(page);
            }
        }
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
            if config_changed {
                // 常规页字段落盘到 weasel.yaml（宿主级设置）；保存后立刻让 broker
                // 重读（1016，毫秒级），不必重启/部署。「保存并应用」随后还会重启
                // 服务，这条通知只是让「仅保存」路径也即时生效。
                match broker::request_settings_reload() {
                    Ok(()) => {}
                    Err(error) => messages.push(format!("宿主配置热更新通知失败：{error}")),
                }
            }
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
                // 成功不弹提示（用户 2026-11-04 10:25：「这个提示删掉」）——
                // 换档下一次按键就生效，不需要告诉用户"打一次字生效"。
                Ok(_) => ui.set_material_status("".into()),
                Err(error) => ui.set_material_status(format!("写材质失败：{error}").into()),
            }
        });
    }

    // 候选窗布局：三个按钮（横流 / 飞瀑 / 竹简），用户 2026-11-05 拍板删掉官方水平/竖直。
    // 启动时若 theme.yaml 还是官方布局，`layout_index()` 返回 -1 ⇒ 三个按钮都不高亮，
    // 点任意一个才切进挤压三模式，不擅自替用户改布局。值走
    // `dialogs::write_theme_style` 同一套 YAML 行编辑，三个布尔与 layout/type 字符串
    // 一起写，避免旧字符串值残留覆盖布尔。
    ui.set_layout_index(dialogs::layout_index(&dialogs::current_layout_value(&user)));
    {
        let ui_weak = ui.as_weak();
        let user = user.clone();
        ui.on_layout_picked(move |index| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let index = (index.max(0) as usize).min(dialogs::LAYOUT_VALUES.len() - 1);
            match dialogs::write_layout_value(&user, dialogs::LAYOUT_VALUES[index]) {
                Ok(_) => ui.set_layout_status("".into()),
                Err(error) => ui.set_layout_status(format!("写布局失败：{error}").into()),
            }
        });
    }
    /// 候选窗字号（2026-11-08 用户要求：放在「常规」页调节）。
    ///
    /// 写用户目录 `theme.yaml` 的 `style/font_point`（`label_font_point` /
    /// `comment_font_point` 同步），主题每次渲染前比对配置时间戳（750ms 缓存），
    /// 改完下一次出现候选窗就重载。界面首次显示与每次切到「常规」页时读一次当前值。
    ui.set_font_point(dialogs::current_font_point(&user) as i32);
    {
        let ui_weak = ui.as_weak();
        let user = user.clone();
        ui.on_font_point_changed(move |value| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let point = value.clamp(
                dialogs::FONT_POINT_MIN as i32,
                dialogs::FONT_POINT_MAX as i32,
            ) as u32;
            match dialogs::write_font_point(&user, point) {
                Ok(_) => {
                    ui.set_font_point(point as i32);
                    ui.set_status(format!("候选窗字号：{point}（下一次出现候选窗即生效）").into());
                }
                Err(error) => ui.set_status(format!("写字号失败：{error}").into()),
            }
        });
    }
    // 每页候选数（2026-11-08 用户要求：放在「常规」页）。
    //
    // 落点是曲线最短的那条真实生效路径：用户目录 `default.yaml` 的 `menu/page_size`。
    // 部署把用户 default.yaml 合并进 `build/*.schema.yaml`，引擎从部署产物读
    // `context.menu.page_size` —— 候选窗分页（`Engine/src/librime/mod.rs`）、翻页键
    // （`menu.rs` 的 paginate）、`select_labels`/`alternative_select_keys` 的数字键
    // 全都消费这一个值，所以不需要另设第二条路径。改完**重新部署后生效**（范围 3–9
    // 保证数字键 1–9 不跨到 0）。
    ui.set_page_size(dialogs::current_page_size(&user, &shared) as i32);
    {
        let ui_weak = ui.as_weak();
        let user = user.clone();
        let shared = shared.clone();
        ui.on_page_size_changed(move |value| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let size = value.clamp(
                dialogs::PAGE_SIZE_MIN as i32,
                dialogs::PAGE_SIZE_MAX as i32,
            ) as u32;
            match dialogs::write_page_size(&user, &shared, size) {
                Ok(_) => {
                    ui.set_page_size(size as i32);
                    ui.set_status(
                        format!("每页候选数：{size}（重新部署后生效，可在「更新」页部署）").into(),
                    );
                }
                Err(error) => ui.set_status(format!("写每页候选数失败：{error}").into()),
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
// ------------------------------------------------------- 方案页（tab 2）

/// 方案页：读可用方案 → 只通过左侧勾选框改 `schema/schema.yaml` → 请 broker 重新部署。
///
/// 数据层与官方 `/switcher` 对话框是同一份（`weasel_style::switcher`），勾选落盘走
/// `write_schema_ids`（保留原有的 `active`）。2026-11-12 用户要求去掉「禁用 / 启用」
/// 按钮：界面不再写 `disabled.yaml`，只保留 `read_disabled()` 兼容外部手工写的存量。
///
/// 取消勾选父方案（如 `pinyin`）只从 `schemas` 移除该父 id；子方案条目、active 与 build
/// 全部保留，子方案因父未选而自动不部署/不进菜单，重新勾选父方案即恢复。
/// 但每个父方案组必须至少留一个**真正可用**（已勾选且未被父未选/禁用压住）的方案——
/// 否则「启用方案」实际什么都没有，界面会在切换时就拒绝并提示。
/// 官方对话框里的快捷键设置这页不做，留一个「官方对话框…」按钮跳过去。
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
    // 子方案 id → 父方案 id（扫描 schema 文件里的父子声明，见 schema_tree）。
    let parents = Rc::new(RefCell::new(BTreeMap::<String, String>::new()));
    // 2026-11-11：禁用态与勾选清单**正交**。勾选写 schema.yaml，禁用写 disabled.yaml；
    // 这里先读盘到内存，点「保存并部署」才落盘（与勾选的「确定才生效」一致）。
    let disabled = Rc::new(RefCell::new(weasel_style::switcher::read_disabled(&user)));
    // 2026-11-12 方案 A：拼音的 16 种输入类型由主方案顶层 input_types 驱动（不再是子方案
    // 文件夹）。这里读出厂清单 + 用户覆盖，作为「父方案组」的组成员，供保留规则与勾选使用。
    let pinyin_input_types = Rc::new(RefCell::new(
        weasel_style::switcher::read_pinyin_input_types(&shared, &user),
    ));
    reload_schemas(
        ui,
        &shared,
        &user,
        &rows,
        &parents,
        &disabled,
        &pinyin_input_types,
    );
    // 运维/回归用：启动时按 id/名字过滤方案列表（例如 pinyin），只显示命中分组。
    // 正常启动不设该变量，列表照旧全量；与 WEASEL_SETTING_PAGE 一样不改用户默认行为。
    if let Ok(filter) = std::env::var("WEASEL_SETTING_SCHEMA_FILTER") {
        set_schema_filter(&filter);
        push_schemas(
            &ui,
            &shared,
            &user,
            &rows,
            &parents,
            &disabled,
            &pinyin_input_types,
        );
    }

    // 勾选/取消：只改内存，点「保存并部署」才写盘（与官方"确定才生效"一致）。
    // 2026-11-12：只有左侧选择框会触发这里（schemas.slint 的 check-touch），整行/名字
    // 都不再可点。子方案也是独立可勾选方案，取消勾选 = 从点名清单移除 = 不编译其 prism。
    //
    // 两道防线：
    //   ① 全局至少留一项；
    //   ② 每个父方案组至少留一个「真正可用」的**子方案**（主方案只是词库/规则外壳，
    //      本身不输入、不出候选，不算可用成员——2026-11-12 晚间修订：保留义务落在
    //      子方案上，取消最后一个可用子方案要被拒绝，且提示点名那个子方案）。
    // 只拒绝「会让原本合法的组变空」的切换（前后对比），外部漂移出来的空组
    // 仍可通过重新勾选父方案/子方案恢复，不会死锁。
    {
        let ui_weak = ui.as_weak();
        let shared = shared.to_path_buf();
        let user = user.to_path_buf();
        let rows = rows.clone();
        let parents = parents.clone();
        let disabled = disabled.clone();
        let pinyin_input_types = pinyin_input_types.clone();
        ui.on_schema_toggle(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            // 拼音输入类型（方案 A：16 种拼式由 input_types 承担，不再是子方案）。
            // 合成 id `__it__<option>`，父方案 pinyin 在 push_schemas 里把它展开成子行。
            if let Some(option) = id.as_str().strip_prefix("__it__") {
                // 2026-11-15：拼音主方案没勾选时，输入类型子行全部灰掉且不可单独改动。
                // 这是「父未选压住」的计算态，不写 disabled.yaml；重勾父方案自动恢复。
                if !rows.borrow().iter().any(|r| r.id == "pinyin" && r.selected) {
                    ui.set_schema_status(
                        "拼音主方案未勾选，输入类型已停用；请先勾选「拼音」。".into(),
                    );
                    return;
                }
                let label = {
                    let pit = pinyin_input_types.borrow();
                    pit.as_ref()
                        .and_then(|pit| pit.options.iter().find(|o| o.option == option))
                        .map(|o| o.label.clone())
                        .unwrap_or_else(|| option.to_string())
                };
                let mut pit = pinyin_input_types.borrow_mut();
                let Some(pit) = pit.as_mut() else { return };
                let currently = pit.enabled.contains(option);
                if currently {
                    // 取消最后一个可用输入类型 ⇒ 拒绝（保留义务落到 input_types 组）。
                    if pit.enabled.len() <= 1 {
                        ui.set_schema_status(
                            format!(
                                "「{}（{}）」是拼音最后一个可用输入类型，不能取消；请先勾选其他输入类型。",
                                label, option
                            )
                            .into(),
                        );
                        return;
                    }
                    pit.enabled.remove(option);
                } else {
                    pit.enabled.insert(option.to_string());
                }
                drop(pit);
                push_schemas(
                    &ui,
                    &shared,
                    &user,
                    &rows,
                    &parents,
                    &disabled,
                    &pinyin_input_types,
                );
                return;
            }
            let rejection = {
                let rows_ref = rows.borrow();
                let Some(current) = rows_ref.iter().find(|row| row.id == id.as_str()) else {
                    return;
                };
                let next = !current.selected;
                let before: BTreeSet<String> = rows_ref
                    .iter()
                    .filter(|row| row.selected)
                    .map(|row| row.id.clone())
                    .collect();
                let mut candidate = before.clone();
                if next {
                    candidate.insert(id.to_string());
                } else {
                    candidate.remove(id.as_str());
                }
                if candidate.is_empty() {
                    Some("至少要保留一个勾选方案。".to_string())
                } else {
                    let group_label = |gid: &str| -> String {
                        rows_ref
                            .iter()
                            .find(|row| row.id == gid)
                            .map(|row| format!("{}（{}）", row.name.as_str(), row.id.as_str()))
                            .unwrap_or_else(|| gid.to_string())
                    };
                    let pre_empty =
                        weasel_style::switcher::parent_groups_without_deployable_member(
                            &before,
                            &disabled.borrow(),
                            &parents.borrow(),
                        );
                    let post_empty =
                        weasel_style::switcher::parent_groups_without_deployable_member(
                            &candidate,
                            &disabled.borrow(),
                            &parents.borrow(),
                        );
                    let newly_empty: Vec<&String> = post_empty
                        .iter()
                        .filter(|group| !pre_empty.contains(*group))
                        .collect();
                    if newly_empty.is_empty() {
                        None
                    } else if next {
                        // 勾选动作反而让组变空只可能发生在：父方案未勾选时勾子方案，
                        // 子方案立刻被「父未选」压住。
                        Some(format!(
                            "「{}」组的父方案未勾选：子方案勾了也会处于「父未选」，请先勾选父方案。",
                            newly_empty
                                .iter()
                                .map(|group| group_label(group))
                                .collect::<Vec<_>>()
                                .join("、")
                        ))
                    } else if newly_empty.len() == 1
                        && parents
                            .borrow()
                            .get(id.as_str())
                            .is_some_and(|parent| parent == newly_empty[0].as_str())
                    {
                        // 取消的是子方案，且它所在组因此空了 ⇒ 它就是最后一个可用子方案。
                        Some(format!(
                            "「{}」是「{}」组最后一个可用子方案，不能取消；请先勾选该组其他子方案。",
                            group_label(id.as_str()),
                            group_label(newly_empty[0])
                        ))
                    } else if newly_empty
                        .iter()
                        .any(|group| group.as_str() == id.as_str())
                    {
                        // 取消的是父方案本身：全部子方案进入「父未选」。
                        Some(format!(
                            "取消父方案「{}」会让该组全部子方案进入「父未选」，整组没有可部署/可进菜单的方案；请保持父方案勾选。",
                            group_label(id.as_str())
                        ))
                    } else {
                        Some(format!(
                            "「{}」组至少要保留一个已勾选子方案：取消后整组没有可部署/可进菜单的方案。",
                            newly_empty
                                .iter()
                                .map(|group| group_label(group))
                                .collect::<Vec<_>>()
                                .join("、")
                        ))
                    }
                }
            };
            if let Some(message) = rejection {
                ui.set_schema_status(message.into());
                return;
            }
            {
                let mut rows = rows.borrow_mut();
                if let Some(row) = rows.iter_mut().find(|row| row.id == id.as_str()) {
                    row.selected = !row.selected;
                }
            }
            push_schemas(
                &ui,
                &shared,
                &user,
                &rows,
                &parents,
                &disabled,
                &pinyin_input_types,
            );
        });
    }
    // 父方案行：点折叠箭头展开/收起子方案（父方案本身仍可勾选）。
    {
        let ui_weak = ui.as_weak();
        let shared = shared.to_path_buf();
        let user = user.to_path_buf();
        let rows = rows.clone();
        let parents = parents.clone();
        let disabled = disabled.clone();
        let pinyin_input_types = pinyin_input_types.clone();
        ui.on_schema_toggle_children(move |id| {
            toggle_collapsed(id.as_str());
            let Some(ui) = ui_weak.upgrade() else { return };
            push_schemas(
                &ui,
                &shared,
                &user,
                &rows,
                &parents,
                &disabled,
                &pinyin_input_types,
            );
        });
    }
    // 搜索：只过滤显示，不改勾选状态。
    {
        let ui_weak = ui.as_weak();
        let shared = shared.to_path_buf();
        let user = user.to_path_buf();
        let rows = rows.clone();
        let parents = parents.clone();
        let disabled = disabled.clone();
        let pinyin_input_types = pinyin_input_types.clone();
        ui.on_schema_filter_changed(move |text| {
            set_schema_filter(text.as_str());
            let Some(ui) = ui_weak.upgrade() else { return };
            push_schemas(
                &ui,
                &shared,
                &user,
                &rows,
                &parents,
                &disabled,
                &pinyin_input_types,
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        let rows = rows.clone();
        let parents = parents.clone();
        let disabled = disabled.clone();
        let user = user.clone();
        let pinyin_input_types = pinyin_input_types.clone();
        ui.on_schema_apply(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            // 2026-11-11：勾选谁就部署谁。子方案不再自动跟随父方案，
            // 用户可以只留主方案、把不想用的双拼逐个取消勾选（不编译、不进菜单）。
            let rows = rows.borrow();
            let ids: Vec<String> = rows
                .iter()
                .filter(|row| row.selected)
                .map(|row| row.id.clone())
                .collect();
            // 官方 OnOK：一项都没选就报错，不写文件。
            if ids.is_empty() {
                ui.set_schema_status("至少要选用一项。".into());
                return;
            }
            let disabled_snapshot = disabled.borrow().clone();
            let selected_ids: BTreeSet<String> = ids.iter().cloned().collect();
            // 2026-11-12 最终闸门：父组必须至少留一个真正可用方案。切换时已经拦过一次，
            // 这里再挡住「外部改过内存/盘后直接点保存」的旁路，不写半状态。
            let empty_groups = weasel_style::switcher::parent_groups_without_deployable_member(
                &selected_ids,
                &disabled_snapshot,
                &parents.borrow(),
            );
            if !empty_groups.is_empty() {
                let names: Vec<String> = empty_groups
                    .iter()
                    .map(|group| {
                        rows.iter()
                            .find(|row| row.id == group.as_str())
                            .map(|row| format!("{}（{}）", row.name.as_str(), row.id.as_str()))
                            .unwrap_or_else(|| group.clone())
                    })
                    .collect();
                ui.set_schema_status(
                    format!(
                        "「{}」组至少要保留一个已勾选子方案；请先勾选该组的一个子方案（主方案外壳本身不算）。",
                        names.join("、")
                    )
                    .into(),
                );
                return;
            }
            let effective = weasel_style::switcher::effective_disabled_ids_from_selection(
                &selected_ids,
                &disabled_snapshot,
                &parents.borrow(),
            );
            let disabled_count = ids.iter().filter(|id| effective.contains(*id)).count();
            let deploy_count = ids.len() - disabled_count;
            let path = user
                .join("schema")
                .join(weasel_style::switcher::SCHEMA_TABLE_FILE);
            // write_schema_ids 保留 active + 全量 schemas。2026-11-12 起设置页不再写
            // disabled.yaml：UI 已无「禁用/启用」按钮，外部手工写的存量禁用态只读保留。
            if let Err(error) = weasel_style::switcher::write_schema_ids(&path, &ids) {
                ui.set_schema_status(format!("写入勾选清单失败：{error}").into());
                return;
            }
            // 2026-11-12 方案 A：拼音输入类型的启用集合同样在此落盘。最后一个可用输入类型
            // 不可取消（切换时已经拦过，这里再挡住「外部改过内存后直接点保存」的旁路）。
            {
                let pit = pinyin_input_types.borrow();
                if let Some(pit) = pit.as_ref() {
                    if pit.enabled.is_empty() {
                        ui.set_schema_status(
                            "拼音至少要保留一个输入类型（明月/地球/自然码…），不能全部取消。".into(),
                        );
                        return;
                    }
                    if let Err(error) = weasel_style::switcher::write_pinyin_input_types_enabled(
                        &user,
                        &pit.enabled,
                        &pit.section,
                    ) {
                        ui.set_schema_status(format!("写入输入类型选择失败：{error}").into());
                        return;
                    }
                }
            }
            ui.set_schema_status(
                format!(
                    "已保存 {} 个勾选（其中 {} 个当前不部署，实际部署 {} 个），正在请 broker 重新部署…",
                    ids.len(),
                    disabled_count,
                    deploy_count
                )
                .into(),
            );
            // 部署必须由掌握引擎生死的 broker 做（引擎单实例，自己拉第二个只会
            // 得到 AlreadyRunning）；与官方对话框 OnOK 之后是同一个动作。
            switcher_dialog::request_deploy();
        });
    }
    {
        let ui_weak = ui.as_weak();
        let rows = rows.clone();
        let parents = parents.clone();
        let disabled = disabled.clone();
        let shared = shared.to_path_buf();
        let user = user.to_path_buf();
        let pinyin_input_types = pinyin_input_types.clone();
        ui.on_schema_refresh(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            reload_schemas(
                &ui,
                &shared,
                &user,
                &rows,
                &parents,
                &disabled,
                &pinyin_input_types,
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        ui.on_schema_open_dialog(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if let Err(error) = open_official("/switcher") {
                ui.set_schema_status(format!("无法打开菜单设置：{error}").into());
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
    parents: &Rc<RefCell<BTreeMap<String, String>>>,
    disabled: &Rc<RefCell<weasel_style::switcher::DisabledTable>>,
    pinyin_input_types: &Rc<RefCell<Option<weasel_style::switcher::PinyinInputTypes>>>,
) {
    match weasel_style::switcher::list_schemas(shared, user) {
        Ok(list) => {
            // 父子声明每次重读时一起扫描；字段解析隔离在 `schema_tree` 里。
            *parents.borrow_mut() = schema_tree::scan_parents(shared, user, &list);
            *rows.borrow_mut() = list;
        }
        Err(error) => ui.set_schema_status(format!("读取方案列表失败：{error}").into()),
    }
    // 禁用态也从盘上重读：外部（CLI / 官方对话框）改过 disabled.yaml 不会漏。
    *disabled.borrow_mut() = weasel_style::switcher::read_disabled(user);
    // 拼音输入类型同样重读（出厂清单 + 用户覆盖）。
    *pinyin_input_types.borrow_mut() =
        weasel_style::switcher::read_pinyin_input_types(shared, user);
    push_schemas(
        ui,
        shared,
        user,
        rows,
        parents,
        disabled,
        pinyin_input_types,
    );
}

/// 树里被折叠的父方案 id（默认全部展开）。
static COLLAPSED: std::sync::Mutex<Option<BTreeSet<String>>> = std::sync::Mutex::new(None);
/// 方案筛选框的当前文字（空串 = 不过滤）。
static SCHEMA_FILTER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn collapsed_snapshot() -> BTreeSet<String> {
    match COLLAPSED.lock() {
        Ok(guard) => guard.clone().unwrap_or_default(),
        Err(poisoned) => poisoned.into_inner().clone().unwrap_or_default(),
    }
}

fn toggle_collapsed(name: &str) {
    let mut guard = match COLLAPSED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let set = guard.get_or_insert_with(Default::default);
    if !set.remove(name) {
        set.insert(name.to_string());
    }
}

fn schema_filter_snapshot() -> String {
    match SCHEMA_FILTER.lock() {
        Ok(guard) => guard.clone().unwrap_or_default(),
        Err(poisoned) => poisoned.into_inner().clone().unwrap_or_default(),
    }
}

fn set_schema_filter(text: &str) {
    match SCHEMA_FILTER.lock() {
        Ok(mut guard) => *guard = Some(text.to_string()),
        Err(poisoned) => *poisoned.into_inner() = Some(text.to_string()),
    }
}

/// 把方案列表推给界面（当前方案从 `schema.yaml` 的 `active` 读）。
///
/// 父子关系来自 schema 文件里的声明（见 `schema_tree`），不再按目录分组：
/// 父方案是普通可勾选方案，子方案缩进一层；没有父级的方案照旧平铺。
fn push_schemas(
    ui: &SettingsWindow,
    shared: &std::path::Path,
    user: &std::path::Path,
    rows: &Rc<RefCell<Vec<weasel_style::switcher::SchemaInfo>>>,
    parents: &Rc<RefCell<BTreeMap<String, String>>>,
    disabled: &Rc<RefCell<weasel_style::switcher::DisabledTable>>,
    pinyin_input_types: &Rc<RefCell<Option<weasel_style::switcher::PinyinInputTypes>>>,
) {
    let active = weasel_style::switcher::read_schema_table(shared, user).active;
    let rows = rows.borrow();
    let parents = parents.borrow();
    let disabled_table = disabled.borrow();
    let mut tree = schema_tree::build_tree(
        &rows,
        &parents,
        &collapsed_snapshot(),
        &schema_filter_snapshot(),
    );
    // 2026-11-12 方案 A：16 个子方案文件夹已删，「拼音」的子行改由 input_types 承担——
    // 在 pinyin 行下注入合成子行 `__it__<option>`（label 来自出厂 options，勾选态来自
    // enabled）。折叠态照父方案处理：pinyin 折叠时不展开合成行。
    let pit_snapshot = pinyin_input_types.borrow().clone();
    if let Some(pit) = pit_snapshot.as_ref() {
        if let Some(pos) = tree
            .iter()
            .position(|node| node.id == "pinyin" && node.level == 0)
        {
            tree[pos].child_count = pit.options.len();
            if !tree[pos].collapsed {
                let mut insert_at = pos + 1;
                // 理论上 pinyin 已无真实子行；防御式跳过（万一外部又声明了父子）。
                while tree
                    .get(insert_at)
                    .is_some_and(|n| n.parent.as_deref() == Some("pinyin"))
                {
                    insert_at += 1;
                }
                let synthetic: Vec<schema_tree::TreeRow> = pit
                    .options
                    .iter()
                    .map(|o| schema_tree::TreeRow {
                        id: format!("__it__{}", o.option),
                        parent: Some("pinyin".into()),
                        child_count: 0,
                        level: 1,
                        collapsed: false,
                    })
                    .collect();
                tree.splice(insert_at..insert_at, synthetic);
            }
        }
    }
    // 勾选集合 = schema.yaml 的 schemas（内部状态）。父方案未勾选时，子方案按
    // schema.parent 自动压住；这是计算态，不写 disabled.yaml，父方案自己仍可重新勾选。
    let selected_ids: BTreeSet<String> = rows
        .iter()
        .filter(|row| row.selected)
        .map(|row| row.id.clone())
        .collect();
    // 拼音主方案没勾选 ⇒ 15 项输入类型子行按「父未选」灰掉（记忆保留、可恢复）。
    let pinyin_selected = selected_ids.contains("pinyin");
    let blocked_by_parent =
        weasel_style::switcher::blocked_children_by_unselected_parents(&selected_ids, &parents);
    // 显式禁用（兼容手工写的 disabled.yaml）∪ 父未选压住的子方案。
    let effective = weasel_style::switcher::effective_disabled_ids_from_selection(
        &selected_ids,
        &disabled_table,
        &parents,
    );
    // 父方案自己没勾、但底下还有子方案保留勾选 → 灰色「未选」态，不是「被禁用」。
    let mut muted_parents = BTreeSet::new();
    for (child, parent) in parents.iter() {
        if !selected_ids.contains(parent)
            && rows.iter().any(|row| row.id == *child && row.selected)
        {
            muted_parents.insert(parent.clone());
        }
    }
    // 当前生效的子方案（引擎/Ctrl+` 选择，设置里只展示）。
    // 生效 id 不在两层树里时按平铺/无生效子方案处理。
    let active_child = active.as_deref().and_then(|id| {
        let node = tree.iter().find(|node| node.level == 1 && node.id == id)?;
        let parent = node.parent.clone()?;
        let name = rows.iter().find(|row| row.id == id)?.name.clone();
        Some((parent, name))
    });
    // 勾选数照旧统计全部已勾选项（含禁用）；实际部署数另算并写进状态栏。
    let selected = rows.iter().filter(|row| row.selected).count();
    let disabled_count = rows.iter().filter(|row| effective.contains(&row.id)).count();
    let deploy_count = rows
        .iter()
        .filter(|row| row.selected && !effective.contains(&row.id))
        .count();
    let model: Vec<SchemaRow> = tree
        .into_iter()
        .filter_map(|node| {
            // 方案 A：拼音输入类型合成行不在 rows 里，直接由 input_types 构造。
            // 勾选态 = enabled.contains(option)；无独立禁用/激活/折叠概念。
            if let Some(option) = node.id.strip_prefix("__it__") {
                let pit = pit_snapshot.as_ref()?;
                let opt = pit.options.iter().find(|o| o.option == option)?;
                return Some(SchemaRow {
                    id: node.id.as_str().into(),
                    name: opt.label.as_str().into(),
                    author: "".into(),
                    description: format!("输入类型（{}）", option).into(),
                    selected: pit.enabled.contains(option),
                    active: false,
                    disabled: false,
                    disabled_by_parent: !pinyin_selected,
                    disabled_by_group: false,
                    is_parent: false,
                    muted: !pinyin_selected,
                    level: 1,
                    child_count: 0,
                    collapsed: false,
                    selectable: true,
                    active_child: "".into(),
                });
            }
            let row = rows.iter().find(|row| row.id == node.id)?;
            let active_child_name = active_child
                .as_ref()
                .filter(|(parent, _)| parent == &node.id)
                .map(|(_, name)| name.as_str())
                .unwrap_or_default();
            // 父方案未勾选压住的子方案：显示「父未选」，不给独立「启用」按钮。
            let disabled_by_parent = blocked_by_parent.contains(&row.id);
            // 兼容外部手工写进 disabled.yaml 的整组禁用。
            let disabled_by_group = node.parent.as_deref().is_some_and(|parent| {
                disabled_table.groups.iter().any(|group| group == parent)
            });
            // 父方案本身是普通可勾选方案；有子方案但自己没勾时画灰色（不是「被禁用」）。
            let is_parent = node.child_count > 0;
            let muted = muted_parents.contains(&row.id);
            Some(SchemaRow {
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
                disabled: effective.contains(&row.id),
                disabled_by_parent,
                disabled_by_group,
                is_parent,
                muted,
                level: node.level as i32,
                child_count: node.child_count as i32,
                collapsed: node.collapsed,
                // 每一行都可勾选：子方案取消勾选后部署器就不会编译它。
                selectable: true,
                active_child: active_child_name.into(),
            })
        })
        .collect();
    ui.set_schema_rows(Rc::new(VecModel::from(model)).into());
    ui.set_schema_status(
        format!(
            "{} 个可用方案 · 已勾选 {} 个（当前不部署 {} 个，实际部署 {} 个）",
            rows.len(), selected, disabled_count, deploy_count
        )
        .into(),
    );
}

// ------------------------------------------------------- 界面与方案（官方对话框）

/// 主界面的「外观…」「方案…」两个按钮。
///
/// 托盘菜单里原来各有一项，用户要求收进设置（「这不是设置的部分吗」）。两条都是
/// 官方模板的 Win32 对话框，见 [`open_official`]。
fn wire_official_dialogs(ui: &SettingsWindow) {
    {
        let weak = ui.as_weak();
        ui.on_open_switcher(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(error) = open_official("/switcher") {
                    ui.set_status(format!("无法打开菜单设置：{error}").into());
                }
            }
        });
    }
    {
        let weak = ui.as_weak();
        ui.on_open_style(move || {
            if let Some(ui) = weak.upgrade() {
                if let Err(error) = open_official("/style") {
                    ui.set_status(format!("无法打开外观设置：{error}").into());
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

/// 统计页里「常用字」两行 + 词云的模型。
///
/// 词云的数据**不是** `stat.bin` 的常用字，而是用户词典里的词条（`commit_count` 当频次）：
/// 前者只有单字，做出来是一堆「的一是了」；后者才是「文件夹 / 部署 / 打包」这种真词。
struct StatWordModels {
    cloud: Vec<CloudWord>,
    cloud_note: slint::SharedString,
    big: Vec<StatWord>,
    mid: Vec<StatWord>,
    small: Vec<StatWord>,
}

fn stat_word_models(words: &[(String, u64)]) -> StatWordModels {
    let (words_big, words_mid, words_small) = split_words(words);
    let (cloud, summary) = weasel_settings::wordcloud::build();
    StatWordModels {
        cloud: cloud
            .into_iter()
            .map(|it| CloudWord {
                word: it.word.as_str().into(),
                size: it.size,
                x: it.x,
                y: it.y,
                advance: it.w,
                tint: it.tint,
                count: it.count.min(i32::MAX as u32) as i32,
            })
            .collect(),
        cloud_note: summary.note().into(),
        big: words_big,
        mid: words_mid,
        small: words_small,
    }
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

    // 常用字两行 + 词云：两个入口（主界面内嵌 / 独立 `/stat`）共用同一份模型。
    let models = stat_word_models(&snapshot.words);
    window.set_stat_words(slint::ModelRc::new(slint::VecModel::from(models.big)));
    window.set_stat_cloud(slint::ModelRc::new(slint::VecModel::from(models.cloud)));
    window.set_stat_cloud_note(models.cloud_note.into());
    window.set_stat_words2(slint::ModelRc::new(slint::VecModel::from(models.mid)));
    window.set_stat_words3(slint::ModelRc::new(slint::VecModel::from(models.small)));

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
    let mut hour_pct: Vec<i32> = hour_keys
        .iter()
        .map(|value| ((*value as f32 / hour_peak) * 100.0).round() as i32)
        .collect();
    // 前 7 天日总量柱接在 24 小时柱后(用户 2026-11-07):越久越淡。
    let week = weasel_settings::stats::load_week_bars();
    let mut hour_alpha: Vec<i32> = vec![100; hour_pct.len()];
    for (pct, alpha) in &week {
        hour_pct.push(*pct);
        hour_alpha.push(*alpha);
    }
    window.set_stat_hour_keys(Rc::new(VecModel::from(hour_pct)).into());
    window.set_stat_hour_alpha(Rc::new(VecModel::from(hour_alpha)).into());
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

        // 常用字两行 + 词云：独立 `/stat` 窗口原来这两块是空的（只有主界面内嵌那份填了），
        // 现在和主界面共用同一个模型构造函数。
        let models = stat_word_models(&snapshot.words);
        ui.set_words(slint::ModelRc::new(slint::VecModel::from(models.big)));
        ui.set_cloud(slint::ModelRc::new(slint::VecModel::from(models.cloud)));
        ui.set_cloud_note(models.cloud_note);
        ui.set_words2(slint::ModelRc::new(slint::VecModel::from(models.mid)));
        ui.set_words3(slint::ModelRc::new(slint::VecModel::from(models.small)));

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
    let mut hour_pct: Vec<i32> = hour_keys
        .iter()
        .map(|value| ((*value as f32 / hour_peak) * 100.0).round() as i32)
        .collect();
    // 前 7 天日总量柱接在 24 小时柱后(用户 2026-11-07):越久越淡。
    let week = weasel_settings::stats::load_week_bars();
    let mut hour_alpha: Vec<i32> = vec![100; hour_pct.len()];
    for (pct, alpha) in &week {
        hour_pct.push(*pct);
        hour_alpha.push(*alpha);
    }
    ui.set_hour_keys(Rc::new(VecModel::from(hour_pct)).into());
    ui.set_hour_alpha(Rc::new(VecModel::from(hour_alpha)).into());
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
// 段位 / 境界 / 名著字数水平 的逻辑已统一抽到框架无关的 `weasel_settings::career`
// （Slint 版与 D2D 外壳共用、有单测；详见 `Setting/src/career.rs`）。
// 下面是主页 stats 直接调用的三个名字的别名（其余名字请直接用 `weasel_settings::career::*`）。
use weasel_settings::career::{book_level, career_level, career_realm};

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

/// 从静态索引拼出下载页的行；索引读不到 / 是空的就返回 `None`（调用方回退到 GitHub 列举）。
///
/// 索引里只有 `repo` / `name` / `schemas` / `branch`，没有 star、语言、pushed_at，
/// 所以这几列留空 —— 宁可少显示，也不要编数据。徽标只能靠本地台账判定「已安装」。
fn rows_from_index(
    index_dir: &std::path::Path,
    user_root: &std::path::Path,
) -> Option<(Vec<RepoRow>, Vec<String>)> {
    let (_, groups) = plum_next::index::load(index_dir).ok()?;
    if groups.iter().all(|(_, recipes)| recipes.is_empty()) {
        return None;
    }

    let installed = weasel_settings::download::load_installed(user_root);
    let mut rows: Vec<RepoRow> = Vec::new();
    let mut cats: Vec<String> = vec!["全部".to_owned()];

    for (category, recipes) in groups {
        // 索引里的 key 可能是多级路径（`Chinese/Mandarin`），展示名优先用索引给的 name
        let cat = if category.name.trim().is_empty() {
            plum_next::index::category_name(&category.key).to_owned()
        } else {
            category.name.clone()
        };
        if !cats.contains(&cat) {
            cats.push(cat.clone());
        }
        for recipe in recipes {
            let short = recipe
                .repo
                .rsplit('/')
                .next()
                .unwrap_or(&recipe.repo)
                .to_owned();
            let initial = short
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_owned());
            let description = if recipe.schemas.is_empty() {
                recipe.name.clone()
            } else {
                format!("{} · {}", recipe.name, recipe.schemas.join(", "))
            };
            let badge = weasel_settings::download::badge_for(installed.get(&recipe.repo), None);
            rows.push(RepoRow {
                name: short.into(),
                description: description.into(),
                clone_url: recipe.clone_url().into(),
                default_branch: recipe.branch.clone().unwrap_or_default().into(),
                stargazers: 0,
                language: "".into(),
                category: cat.clone().into(),
                initial: initial.into(),
                badge: badge.into(),
                pushed_at: "".into(),
            });
        }
    }

    Some((rows, cats))
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
            ui.set_download_list_status("正在加载方案列表...".into());
            let ui_weak2 = ui_weak.clone();
            let user_root_f = user_for_list.clone();
            std::thread::spawn(move || {
                // 索引优先：静态索引读得通就不打 GitHub API（离线、限流、断连都能列）。
                // 读不到（还没生成 / 被删 / 解析失败）就照旧回退到组织实时列举，老环境不受影响。
                if let Some((rows, cats)) =
                    rows_from_index(&plum_next::index::default_dir(), &user_root_f)
                {
                    let total = rows.len();
                    let _ = ui_weak2.upgrade_in_event_loop(move |ui| {
                        ui.set_download_repos(Rc::new(slint::VecModel::from(rows)).into());
                        let cats_shared: Vec<slint::SharedString> =
                            cats.iter().map(|c| c.clone().into()).collect();
                        ui.set_download_categories(Rc::new(slint::VecModel::from(cats_shared)).into());
                        ui.set_download_list_status(
                            format!("共 {total} 个仓库（来源：静态索引）").into(),
                        );
                        ui.set_download_net_error("".into());
                    });
                    return;
                }
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
                                    ..Default::default()
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
    use weasel_settings::career::{book_level, format_chars};

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
        use weasel_settings::career::{upcoming_books, BOOK_LEVELS};
        let mut last = 0usize;
        for total in (0..=1_000_000).step_by(500) {
            let upcoming = upcoming_books(total);
            if upcoming.is_empty() {
                continue;
            }
            let (base, name) = upcoming[0];
            let index = weasel_settings::career::BOOK_LEVELS.iter().position(|(_, n)| *n == name).unwrap();
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
