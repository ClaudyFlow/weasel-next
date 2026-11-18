# 锈狼嚎（Weasel-Next）输入法

基于 中州韵输入法引擎 / Rime Input Method Engine 的 Windows 输入法

项目主页：[https://github.com/ClaudyFlow/Weasel-Next](https://github.com/ClaudyFlow/Weasel-Next)

项目使用 [BSD-3-Clause](LICENSE) 开源许可，

本项目基于诸多开源组件及技术，其许可清单见 [第三方许可清单](THIRD-PARTY-LICENSE.txt)。

下载最新版本：[Releases](https://github.com/ClaudyFlow/Weasel-Next/releases)

## 安装与使用

软件适用于 Windows 10 (1809 版本及以上) ~ Windows 11，
部分功能要求具有 Windows 10 (版本 1903) 或更新版本的操作系统。

安装完成后，选择*输入法指示器*中的 Rime 图标，开始使用锈狼嚎进行输入。
右键输入法状态指示器 / 系统*托盘区* Rime 图标，
可通过设置 / 用户文件夹对锈狼嚎及 Rime 输入引擎进行自定义配置。

Rime 配置决定输入法的输入方案与交互，见 [Rime定制指南](https://github.com/rime/home/wiki/CustomizationGuide) 进行配置。

锈狼嚎的配置决定输入法的外观、与操作系统和应用的交互方式，见程序附带的【设置】应用。
输入方案清单不写死在源码里：首次安装部署时由程序根据已安装的方案自动生成到
`<用户文件夹>/schema/schema.yaml`，之后由设置界面与输入法自身维护。

锈狼嚎 **不兼容** 小狼毫 (C/C++版) 的配置文件 (weasel.toml/weasel.custom.toml) 。

## 自定义皮肤

锈狼嚎通过 wasm 提供自定义皮肤接口，但目前并不保证接口稳定。
`theme/` 下包括主题契约与自带主题源码，可供参考。

## 构建

> 仓库结构（2026-11-18）：`librime-next/` 与 `plum-next/` 是真正的 git submodule，
> 克隆时用 `git clone --recurse-submodules`。构建脚本、部署产物、备份与截图不入库，
> 已移到仓外归档；下方历史命令里的 `script/` 步骤仅适用于保留脚本的旧工作区。

开发构建：

```powershell
# 首先下载构建所需要的组件
.\script\download_librime.ps1
.\script\download_vcredist.ps1
cargo build --workspace --release
```

发布构建：

```powershell
# 构建输入法及安装包
.\script\build-release.ps1
.\script\build-installer.ps1
```

## 致谢

本项目建立在以下项目与作者的工作之上：

- **Rime / 小狼毫原版**：[Rime](https://rime.im) 社区，以及 [librime](https://github.com/rime/librime) 引擎与 [Weasel（小狼毫）](https://github.com/rime/weasel) 前端。本项目的引擎语义、配置体系与候选窗行为均以原版为基准。
- **fxliang**：[weasel](https://github.com/fxliang/weasel) 的 C++ UI 与渲染链、[rime](https://github.com/fxliang/rime) 的编译图与词典实现、[rime-cli](https://github.com/fxliang/rime-cli) 的进度条/代理参数、[rime-plugin-template](https://github.com/fxliang/rime-plugin-template) 的插件接口约定，是多处实现的直接参照。
- **amzxyz**：[万象拼音](https://github.com/amzxyz/rime-wanxiang) 与 librime-evo（[rime/librime PR #1232](https://github.com/rime/librime/pull/1232)），为拼音方案、棱镜/词库组织与 `input_types` 设计提供参考。
- **水杉输入法（MSIME）**：[metasequoiaime/msime](https://github.com/metasequoiaime/msime) 及其作者。感谢开放交流，以及 [issues / PR](https://github.com/metasequoiaime/msime/issues) 中关于学习同步、词库自改进闭环与 TSF 前端防闪的记录，部分设计灵感由此而来；整理见 [水杉开放需求清单](doc/shuishan-msime-requirements.md)。
