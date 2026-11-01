# lotem/typewriter 学习笔记（Rime 打字機）

> 2026-10-24。源码：`C:\Code\typewriter`（`git clone` 走不通 github:443，用
> `https://codeload.github.com/lotem/typewriter/tar.gz/refs/heads/master` 下的，95 KB）
> 在线版：<https://rime.io/typewriter>
> 作者 **lotem = GONG Chen**，也就是 librime / Rime 的作者本人。

## 一句话

**一个纯 Rust（Leptos + WASM）的网页打字练习器，内置「Rime 微观引擎」** ——
在浏览器里实现了输入法的输入状态机：多方案、并击/连击触键、虚拟键盘、练习题库。
**7,018 行 Rust**，比 librime 的 `gear/` + 顶层（17,039 行 C++）小得多，因为它只做"打字"这一条路。

## 对我们（librime-next）的三点直接价值

### ① `gear`/`engine` 层的**架构参考**（不是逐行移植对象）

它把引擎拆成一组「**機關**」(mechanism) 串联，每个机关是纯函数式的 `(输入信号, 环境) → 输出信号`：

```
輸入方案機關  ← 当前方案 / 切换
佈局機關      ← 键盘配列（qwerty / alphabet / ortho / ortho_split / columnar_split …）
作業機關      ← 练习题（drill）状态机：打字中 → 完成 → 退格重练 → 自习
字幕機關      ← 把要打的文本变成"目标字幕"
連擊機關      ← 连击（依次按键）
並擊機關      ← 并击（和弦，chord）
工作模式機關  ← 练习 / 自习 / 自由
```

对比 librime：那边是 `translator` / `filter` / `segmentor` / `processor` 四类齿轮；
这边的「机关」更粗、更聚焦，**没有候选窗/排序/语言模型** —— 所以它替代不了我们的 `config`/`dict` 层，
但**能当我们设计 `gear` 时的活样本**：至少证明"用 Rust 写输入状态机"是可行的，且 7 千行就够用。

### ② `spelling_algebra.rs` —— 和我们 `algo/algebra.rs` 是同一件东西

```rust
pub enum 拼寫運算<'a> {
    變換 { 模式: MaybeOwned<'a, Regex>, 替換文字: &'a str },   // = Rime 的 xform
    轉寫 { 字符映射: HashMap<char, char> },                    // = xlit
    消除 { 模式: MaybeOwned<'a, Regex> },                     // = erase
}
```

⇒ **可以交叉验收**：同一批规则分别喂给它和我们 `algo/algebra.rs`，比输出是否一致。
它用 `lazy_regex`（真 PCRE-ish 引擎），我们用的是自写子集 —— 正好可以拿它当"标准答案"补我们的正则缺口
（尤其 `recognizer/patterns` 里那两条环视/懒惰量词）。

### ③ 方案即数据、且**编译期检查**

`theory/combo_pinyin.rs` 里方案是 Rust 常量 + 宏，不是 YAML：

```rust
const 並擊鍵序: &[鍵位定義] = &[
    鍵位!(S => KeyCode::S), 鍵位!(C => KeyCode::W), …
];
```

对比我们的 YAML 方案：好处是错字/键名在编译期就报；坏处是改方案要重编。
**这给了我们一个可选项**：`librime-next` 内部方案也可以用 Rust 常量表表达，
YAML 只作为外部交换格式（编译成常量表后跑）。

## 其它值得记的

* **中文标识符**：`變換` / `輸入方案機關` / `並擊機關` …… 整个 crate 用中文命名。
  这是"中文变量名"在真实项目里的先例（用户 2026-10-24 问过 YAML 能不能用中文键）。
* **触键方式抽象**：`並擊`(chord) / `連擊` / `單擊` 三种，用 trait/枚举统一，
  对我们将来的 `key_binder` 与组合键处理有参考。
* **URL 单一真值来源 + IoC**（`docs/architecture_journey.md`）：
  URL 是初始遥控器，Engine 处理复杂交互，Effect 反向同步 —— 对 Setting UI 的状态同步有借鉴。
* **不做什么**：没有候选窗、没有排序、没有 `.bin` 词典、没有 LevelDB、没有 `__include`/`__patch`。
  ⇒ **它不是 librime 的替代品，是"输入状态机"这一段的参考实现**。

## 结论

| 用途 | 能不能用 |
|---|---|
| 替代 `config` / `dict` 层 | ❌ 不涉及 |
| **`gear`/`engine` 的架构与状态机设计参考** | ✅ 强烈建议读 |
| **`algebra` 正则对拍的"标准答案"** | ✅ 直接可用 |
| 方案定义的"Rust 常量表"思路 | ✅ 可选方案 |
| 中文标识符先例 | ✅ 已有先例 |
