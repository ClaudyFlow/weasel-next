# 皮肤格式对比示例

同一个 steam 主题,用三种标准各写一遍。看完选一种,告诉甲路/乙路会话切到哪个。

| 文件 | 格式 | 特点 |
|---|---|---|
| `skin-hufu-format.json` | hufu JSON | RGBA 含 alpha 通道、毛玻璃(black 半透明 back)、胶囊颜色、阴影半径/偏移；**19 个内置皮肤可对比** |
| `skin-msime-format.toml` | msime TOML v2 | DIP 单位(高 DPI 自适应)、9-patch 贴图、canvas 基底、decorations 挂件(支持 APNG/GIF 动图)；**表达力最强** |
| `skin-current-yaml.yaml` | weasel YAML(现行) | 只有配色(无 alpha、无贴图、无 DIP)；**表达力最弱**但最简单 |

## 差异速览

| 能力 | hufu JSON | msime TOML | 现行 YAML |
|---|---|---|---|
| 颜色 + alpha | ✅ 8 位 HEX | ✅ 8 位 HEX | ❌ 6 位 HEX(无透明度) |
| 圆角 | ✅ corner_radius | ✅ item_corner_radius_dip | ❌ 无 |
| 9-patch 贴图 | ❌ | ✅ border_image | ❌ |
| 挂件(动图/图标) | ❌ | ✅ decorations | ❌ |
| DIP 高 DPI | ✅ f64 | ✅ _dip 后缀 | ❌ 像素 |
| 阴影 | ✅ radius+offset | ✅ shadow_radius | ❌ 只有 shadow_color |
| 毛玻璃 | ✅ alpha back | ✅ canvas 基底 | ❌ |
| 字体/字号/间距 | ✅ | ✅ | ✅(最全) |

## 推荐

- **要毛玻璃/圆角/透明度** → hufu JSON(改动最小,加 alpha + 圆角即可)
- **要贴图/挂件/动图/DPI 完美** → msime TOML v2(表达力最强,但需要美术资产)
- **只改颜色** → 现行 YAML 够用,不用换
