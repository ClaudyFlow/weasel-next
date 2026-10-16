# 配置描述文件

- `xxx.toml`：默认配置的唯一来源（手写，不再由脚本生成）。
- `xxx.richschema.json`：人工维护的标准约束和界面提示（设置界面按它渲染）。

原先还有一份 `xxx.schema.json`（给编辑器用的标准 JSON Schema），由
`script/generate-schemas.mjs` 从上面两份生成；**该生成器与生成物都已删除** ——
默认值只有 `xxx.json` 一处，避免"同一份默认值维护两遍"。

richschema 的 `formatVersion` 当前为 1；`schema` 使用标准 JSON Schema，
`ui.fields` 以主题局部的 JSON Pointer 为键，存放排序、控件、分组等界面提示。
字段名称、说明仍放在 schema 的 title/description，不在两处重复维护。
后续条件显示采用声明式规则，不允许描述文件执行脚本或外部命令。

主题描述不包含宿主配置路径；主题目录注册与实际挂载位置由设置应用负责。
本次仅迁移描述来源和生成流程，尚未实现动态表单及主题目录发现。
