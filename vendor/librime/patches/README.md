# vendor/librime 头文件偏离记录(patches 纪律)

参照 fxliang/rime-plugin-template 的补丁纪律:模板保持"载荷中性",我们这里是
**Rust 引擎自有的 C ABI 头**,作为宿主(Weasel-Next-Engine.exe 的 bindgen)与
`librime-next` c_api 导出表之间的**唯一契约**。每次对上游 rime_api.h 的偏离必须
在本文件登记——否则下次从上游同步头文件时会**静默冲掉扩展**,宿主 bindgen 少字段
不报错,运行时才炸。

## 当前偏离(截至 2026-11-08)

### 1. `commit_string`(新增,rime_api.h:319 附近)

```c
Bool (*commit_string)(RimeSessionId session_id, const char* text);
```

- 位置:`commit_composition` 之后、`clear_composition` 之前(**顺序敏感**——
  Rust 导出表在 `librime-next/src/c_api.rs` 的 `RimeApi` struct 字段序必须与本
  头文件逐字段一致,这是 ABI)。
- 用途:直拟上屏(符号面板/剪贴板卡片点击)——不经组合直接提交 UTF-8 文本,
  宿主 `process_renderer_event` 的 `COMMIT_TEXT` 臂经 `commit_string` 写入
  `Context::commit_text`,`get_commit` 取走。
- 对应 Rust 侧:`c_api.rs` 的 `commit_string` 字段 + `rime_commit_string`;
  `Session::commit_string`(组字中先 `commit_composition` 再补提交)。

### 2. `RimeContext.predictions` / `predictions_len`(新增,`RimeContext` 尾部)

```c
char** predictions;      // null 结尾的 UTF-8 词数组,由 librime 持有,free_context 释放
int    predictions_len;  // 词数
```

- 位置:`RimeContext` 结构体**最末尾**(5 个官方字段之后)。`RIME_STRUCT_INIT`
  的 `data_size` 口径是 `sizeof(Type) - sizeof(data_size)`(与宿主
  `struct_data_size()` 同口径)。
- 用途:**上屏后联想**——`librime-next/src/engine/runtime.rs::apply_commit` 在
  学习之后用 `user_ngram::predictions_for_context` 填「本人上屏 ≥10 次的多字
  中文词」(最多 3 条),宿主 Engine 的 `read_response` 读走后变成渲染快照的
  `kind="prediction"` items,主题在无组字时画联想条。
- ABI 安全规则(2026-11-03 越界事故的反向教训):
  - exe 与 dll **必须同批部署**:新 dll + 旧宿主安全(`rime_get_context` /
    `rime_free_context` 按调用方 `data_size` 守卫,不够大一个字节都不碰);
    新宿主 + 旧 dll 也安全(旧 dll 不写,字段保持 0)。
  - 守卫门槛必须写 `>= sizeof(RimeContext) - sizeof(int)`;**写成
    `>= sizeof(RimeContext)` 会让扩展字段永远送不出去**(2026-11-07 第二断点)。
- 对应 Rust 侧:`engine/context.rs` 字段、`engine/runtime.rs::apply_commit`、
  `engine/user_ngram.rs::{predict_frequent, predictions_for_context}`、
  `c_api.rs::{rime_get_context, rime_free_context}` 的守卫。

## 同步上游头文件时的检查单

1. `diff` 新旧 rime_api.h,确认上表两条偏离是否还在;
2. `grep commit_string vendor/librime/include/rime_api.h` 必须 1 处;
3. `grep -n "predictions" vendor/librime/include/rime_api.h` 必须命中
   `RimeContext` 的 `char** predictions; int predictions_len;`(**且必须在结构体
   末尾**——插在中间会改掉后续字段偏移);
4. `cargo build -p librime-next -p Weasel-Next-Engine` 同时过(bindgen 少字段
   会在 Engine 侧报 no field,不会静默);
5. **显式 build,不要 cargo test 后直接拷 dll**(test 不重编 cdylib,见
   `panel-dock-commit-state` 记忆的部署铁律);
6. 部署前核对 `dll mtime > 源码 mtime`;
7. 头文件有 `RimeContext` 扩展时,**exe 与 dll 必须同批部署**并在部署后核
   `data_size` 守卫(见上表第 2 条 ABI 安全规则)。

## 历则(对齐 fxliang/rime-plugin-template 的纪律)

* 模板原则"补丁仅限构建适配,载荷保持中性"——我们不构建 C++ librime(引擎是
  Rust),本 vendor 目录只有头文件,所以偏离性质是 **ABI 扩展**,每条都要能说出
  用途和消费方(见上表"用途"列)。
* 上游没有的符号一律登记;上游有的符号禁止改签名(会同时炸宿主 bindgen 与
  c_api 导出表)。
