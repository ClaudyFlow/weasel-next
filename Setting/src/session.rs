//! 一次设置会话：把配置文档、主题目录、表单字段与界面视图绑在一起。
//!
//! 设置前端用这里的能力处理"加载 → 改字段 → 校验 → 保存"。所有写入
//! 先进入内存中的用户补丁，只有 [`Session::save`] 会写盘（原子替换 + 外部修改检测）。
//!
//! 视图结构（[`View`] 及其子结构）直接派生 `Serialize`，便于前端直接使用
//! JSON 返回；Slint 前端不需要这些类型，只调用会话方法即可。
use serde::Serialize;
use serde_json::{json, Value};
use weasel_common::process::RuntimePaths;

use crate::{app_options, catalog, config::Document, form};

/// 表单字段视图：`form::Field` 的可序列化投影。
#[derive(Serialize)]
pub struct FieldView {
    /// 写入覆盖配置时使用的完整 JSON 路径（含分区挂载路径）。
    pub path: Vec<String>,
    /// 字段本地键名。
    pub key: String,
    pub title: String,
    pub description: String,
    /// 控件类别：`color` / `enum_number` / `choice` / `string` / `json`。
    pub kind: String,
    /// 选项控件可选值；`selected == i + 1` 时取 `values[i]`。
    pub values: Vec<Value>,
    /// 界面显示项：索引 0 恒为"继承"，末项可能是"自定义"或"覆盖"。
    pub choices: Vec<String>,
    /// 当前覆盖在选项中的位置；0 = 未覆盖。
    pub selected: i32,
    /// 控件应显示的位置（继承状态映射到继承值的位置）。
    pub display_selected: i32,
    /// 字符串/颜色/JSON/自定义数值控件的编辑文本。
    pub text: String,
    pub boolean: bool,
    pub multiline: bool,
    pub inherited_text: String,
    pub inherited_selected: i32,
    pub allow_system_color: bool,
    pub adaptive_color: bool,
    /// 是否已被用户覆盖。
    pub overridden: bool,
}

/// 一个设置分区（常规 / 各主题 / WASM 模块）。
#[derive(Serialize)]
pub struct SectionView {
    pub title: String,
    pub mount: Vec<String>,
    pub fields: Vec<FieldView>,
    /// 该分区最终生效的 JSON 值。
    pub effective: Value,
}

/// 按应用覆盖项视图。
#[derive(Serialize)]
pub struct AppOptionView {
    pub app: String,
    pub ascii_mode: Option<bool>,
    pub inline_preedit: Option<bool>,
    /// 用户在补丁里显式写过的键。
    pub overridden: Vec<String>,
}

/// 配置与运行时路径。
#[derive(Serialize)]
pub struct PathsView {
    pub config: String,
    pub config_exists: bool,
    pub executable_directory: String,
    pub user_data: String,
    pub logs: String,
}

/// 前端一次加载所需的全部数据。
#[derive(Serialize)]
pub struct View {
    pub sections: Vec<SectionView>,
    pub notices: Vec<String>,
    pub paths: PathsView,
    /// 是否有未保存的改动。
    pub dirty: bool,
    pub app_options: Vec<AppOptionView>,
}

/// 一次设置会话的完整状态。
pub struct Session {
    document: Document,
    catalog: catalog::Catalog,
    /// 与 `catalog.sections` 一一对应的字段模型。
    fields: Vec<Vec<form::Field>>,
    dirty: bool,
}

impl Session {
    /// 加载配置与目录，并为每个分区生成字段模型。
    pub fn open(paths: &RuntimePaths) -> Result<Self, String> {
        let document = Document::load(paths)?;
        let catalog = catalog::load(paths, &document)?;
        let mut session = Self {
            document,
            catalog,
            fields: Vec::new(),
            dirty: false,
        };
        session.refresh_fields()?;
        Ok(session)
    }

    /// 重新生成字段模型（改动或重载之后调用）。
    pub fn refresh_fields(&mut self) -> Result<(), String> {
        let mut fields = Vec::with_capacity(self.catalog.sections.len());
        for section in &self.catalog.sections {
            fields.push(form::fields(section, &self.document)?);
        }
        self.fields = fields;
        Ok(())
    }

    /// 已加载的配置文档。
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// 已加载的主题目录。
    pub fn catalog(&self) -> &catalog::Catalog {
        &self.catalog
    }

    /// 指定分区的字段模型。
    pub fn fields(&self, section: usize) -> Option<&[form::Field]> {
        self.fields.get(section).map(Vec::as_slice)
    }

    /// 是否有未保存改动。
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// 应用一个字段改动：`selected == 0` 表示移除覆盖、恢复继承。
    ///
    /// 值由 [`form::Field::value`] 解析（含自定义数值与 JSON 文本校验），成功后写入补丁。
    pub fn apply(
        &mut self,
        section: usize,
        field: usize,
        selected: i32,
        text: &str,
    ) -> Result<(), String> {
        let model = self
            .fields
            .get(section)
            .and_then(|fields| fields.get(field))
            .ok_or("字段不存在")?;
        let value = model.value(selected, text)?;
        let path = model.path.clone();
        form::assign(&mut self.document.patch, &path, value)?;
        self.dirty = true;
        self.refresh_fields()
    }

    /// 校验每个分区的最终生效值并原子保存用户补丁。
    pub fn save(&mut self) -> Result<(), String> {
        for section in &self.catalog.sections {
            section
                .metadata
                .validate(&form::effective(section, &self.document))?;
        }
        self.document.save()?;
        self.dirty = false;
        self.refresh_fields()
    }

    /// 清空用户补丁（恢复全部默认值；仍需 [`Session::save`] 才写盘）。
    pub fn reset(&mut self) -> Result<(), String> {
        self.document.patch = json!({});
        self.dirty = true;
        self.refresh_fields()
    }

    /// 丢弃内存改动：从磁盘重新加载。
    pub fn discard(&mut self, paths: &RuntimePaths) -> Result<(), String> {
        *self = Self::open(paths)?;
        Ok(())
    }

    /// 新增按应用覆盖项。
    pub fn app_add(&mut self, app: &str) -> Result<(), String> {
        app_options::add(&mut self.document, app)?;
        self.dirty = true;
        self.refresh_fields()
    }

    /// 删除按应用覆盖项。
    pub fn app_remove(&mut self, app: &str) -> Result<(), String> {
        app_options::remove(&mut self.document, app)?;
        self.dirty = true;
        self.refresh_fields()
    }

    /// 修改某个应用的受支持选项；`value` 为 `None` 时移除该覆盖。
    pub fn app_set(&mut self, app: &str, key: &str, value: Option<bool>) -> Result<(), String> {
        app_options::edit(&mut self.document, app, key, value)?;
        self.dirty = true;
        self.refresh_fields()
    }

    /// 构造前端视图。
    pub fn view(&self, paths: &RuntimePaths) -> Result<View, String> {
        let mut sections = Vec::with_capacity(self.catalog.sections.len());
        for (index, section) in self.catalog.sections.iter().enumerate() {
            let model = self.fields.get(index).ok_or("字段模型与目录不同步")?;
            let fields = model
                .iter()
                .map(|field| FieldView {
                    path: field.path.clone(),
                    key: field.key.clone(),
                    title: field.title.clone(),
                    description: field.description.clone(),
                    kind: field.kind.clone(),
                    values: field.values.clone(),
                    choices: field.choices.clone(),
                    selected: field.selected,
                    display_selected: field.display_selected(),
                    text: field.text.clone(),
                    boolean: field.boolean,
                    multiline: field.multiline,
                    inherited_text: field.inherited_text.clone(),
                    inherited_selected: field.inherited_selected,
                    allow_system_color: field.allow_system_color,
                    adaptive_color: field.adaptive_color,
                    overridden: field.selected != 0,
                })
                .collect();
            sections.push(SectionView {
                title: section.title.clone(),
                mount: section.mount.clone(),
                fields,
                effective: form::effective(section, &self.document),
            });
        }

        let effective = self.document.effective();
        let mut options = Vec::new();
        if let Some(apps) = effective["app_options"].as_object() {
            for (app, value) in apps {
                let overridden = self.document.patch["app_options"][app]
                    .as_object()
                    .map(|object| object.keys().cloned().collect())
                    .unwrap_or_default();
                options.push(AppOptionView {
                    app: app.clone(),
                    ascii_mode: value["ascii_mode"].as_bool(),
                    inline_preedit: value["inline_preedit"].as_bool(),
                    overridden,
                });
            }
        }

        let config = paths.user_data.join("weasel.custom.yaml");
        Ok(View {
            sections,
            notices: self.catalog.notices.clone(),
            paths: PathsView {
                config: config.display().to_string(),
                config_exists: config.exists(),
                executable_directory: paths.executable_directory.display().to_string(),
                user_data: paths.user_data.display().to_string(),
                logs: paths.logs.display().to_string(),
            },
            dirty: self.dirty,
            app_options: options,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取运行时路径；测试环境拿不到就跳过（只读，不写用户配置）。
    fn paths() -> Option<RuntimePaths> {
        RuntimePaths::discover().ok()
    }

    #[test]
    fn open_yields_sections_fields_and_view() {
        let Some(paths) = paths() else {
            eprintln!("无法确定运行时路径，跳过");
            return;
        };
        let session = Session::open(&paths).expect("加载配置应成功");
        assert!(!session.catalog().sections.is_empty(), "至少要有常规分区");
        let total: usize = (0..session.catalog().sections.len())
            .map(|index| session.fields(index).map(<[form::Field]>::len).unwrap_or(0))
            .sum();
        assert!(total > 0, "schema 应至少产出一个字段");

        let view = session.view(&paths).expect("生成视图");
        assert_eq!(view.sections.len(), session.catalog().sections.len());
        assert!(!view.paths.config.is_empty());
        assert!(view.paths.config.ends_with("weasel.custom.yaml"));
        assert!(!view.dirty);
    }

    #[test]
    fn apply_then_reset_only_touches_the_in_memory_patch() {
        let Some(paths) = paths() else {
            return;
        };
        let mut session = Session::open(&paths).expect("加载配置");
        let found = (0..session.catalog().sections.len()).find_map(|section| {
            session
                .fields(section)
                .and_then(|fields| {
                    fields
                        .iter()
                        .position(|field| field.kind == "choice" && field.values.len() == 2)
                })
                .map(|field| (section, field))
        });
        let Some((section, field)) = found else {
            eprintln!("没有布尔选择字段，跳过");
            return;
        };

        let path = session.fields(section).unwrap()[field].path.clone();
        session.apply(section, field, 1, "").expect("写入覆盖");
        assert!(session.dirty());
        assert!(form::at(&session.document().patch, &path).is_some());
        assert!(session.view(&paths).unwrap().dirty);
        assert!(session.fields(section).unwrap()[field].selected != 0);

        // 恢复继承
        session.apply(section, field, 0, "").expect("移除覆盖");
        assert!(form::at(&session.document().patch, &path).is_none());
        assert!(session.fields(section).unwrap()[field].selected == 0);

        // 清空全部覆盖（同样不写盘）
        session.reset().expect("清空补丁");
        assert_eq!(session.document().patch, json!({}));
    }

    #[test]
    fn invalid_field_index_is_reported() {
        let Some(paths) = paths() else {
            return;
        };
        let mut session = Session::open(&paths).expect("加载配置");
        assert!(session.apply(999, 0, 1, "").is_err());
        assert!(session.apply(0, 999, 1, "").is_err());
    }
}
