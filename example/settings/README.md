# 设置文件格式对比

同一家(Weasel-Next)的**全套设置**，用三种标准各写一遍。看完选一种，告诉甲/乙会话。

| 文件 | 格式 | 来源项目 | section 数 |
|---|---|---|---|
| `settings-hufu-format.json` | hufu JSON(13 section) | 虎符 | general/schema/input/candidates/reverse/sentence/punct/clipboard/appearance/sound/opencc/user/keymap |
| `settings-msime-format.toml` | msime TOML(9 section) | 水杉 | general/switch/schema/input/candidates/sentence/reverse_lookup/punctuator/appearance/opencc/user_dict/keymap |
| `settings-weasel-format.yaml` | weasel YAML(现行) | 小狼毫 | switch/menu/style/schemes/key_binder/recognizer |

## 覆盖面速览

| 设置域 | hufu JSON | msime TOML | 现行 YAML |
|---|---|---|---|
| 开机自启 | ✅ | ✅ | ❌(Run 键) |
| 中英切换(Shift/Caps/Ctrl+Space) | ✅(含 CapsAction 枚举) | ✅ | ❌(写死) |
| 方案切换/记忆 | ✅(recent_pair/export_formats) | ✅ | ❌ |
| 输入行为(满码/清屏/混合) | ✅(15 项) | ✅(8 项) | ❌(散在方案) |
| 模糊音 | ❌ | ✅(fuzzy_pinyin_sets) | ❌ |
| 候选窗(页大小/横竖/emoji) | ✅ | ✅ | ✅(menu/) |
| 整句/语言模型权重 | ✅(ngram+grammar+neural) | ✅ | ❌ |
| 反查 | ✅ | ✅ | ✅(recognizer/) |
| 标点 | ✅ | ✅ | ✅(punctuator import) |
| 剪贴板 | ✅ | ❌ | ❌ |
| 外观/皮肤 | ✅(引用 skin 文件) | ✅(引用 skin 文件) | ✅(style+color_scheme) |
| 按键音效 | ✅(4 项) | ❌ | ❌ |
| opencc 简繁 | ✅ | ✅ | ✅ |
| 用户词典 | ✅(learning/decay) | ✅ | ❌ |
| 按键绑定 | ✅(keymap.bindings) | ✅(keymap.bindings) | ✅(key_binder/) |

## 推荐

- **设置面最全 → hufu JSON**:13 个 section 覆盖全部体验，设置 UI 直接读写不碰 yaml
- **结构最清晰 → msime TOML**:按域分 section，可读性最好，TOML 类型安全
- **兼容性最强 → weasel YAML**:与所有 Rime 生态共用，C++ 引擎直接吃

## 选型建议

如果你想在 Weasel-Next 里做一个**图形化设置页**（像 hufu 那样不碰 yaml/lua），
推荐 **hufu JSON 格式**——它的 13 个 section 就是一个设置页的 13 个标签页，
每个字段都有注释和默认值，直接映射到 Slint UI 控件。
