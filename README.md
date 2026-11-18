# 锈狼嚎（Weasel-Next）输入法

基于 中州韵输入法引擎 / Rime Input Method Engine 的 Windows 输入法

项目主页：[https://rime.im](https://rime.im)

项目使用 [BSD-3-Clause](LICENSE) 开源许可，

本项目基于诸多开源组件及技术，其许可清单见 [第三方许可清单](THIRD-PARTY-LICENSE.txt)。

下载最新版本：[Releases](https://github.com/determ1ne/weasel-next/releases)

## 安装与使用

软件适用于 Windows 10 (1809 版本及以上) ~ Windows 11，
部分功能要求具有 Windows 10 (版本 1903) 或更新版本的操作系统。

安装完成后，选择*输入法指示器*中的 Rime 图标，开始使用小狼毫进行输入。
右键输入法状态指示器 / 系统*托盘区* Rime 图标，
可通过设置 / 用户文件夹对小狼毫及 Rime 输入引擎进行自定义配置。

Rime 配置决定输入法的输入方案与交互，见 [Rime定制指南](https://github.com/rime/home/wiki/CustomizationGuide) 进行配置。

小狼毫配置决定输入法的外观、与操作系统和应用的交互方式，见程序附带【设置】应用或源码中的配置文件 schema 进行配置。

锈狼嚎 **不兼容** 小狼毫 (C/C++版) 的配置文件 (weasel.toml/weasel.custom.toml) 。

## 自定义皮肤

小狼毫通过 wasm 提供自定义皮肤接口，但目前并不保证接口稳定。
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

- [水杉输入法（MSIME）](https://github.com/metasequoiaime/msime) 及其作者——**本项目的部分设计灵感来源于水杉输入法**：
  感谢作者与本项目的开放交流，以及其仓库里详实的
  [功能需求与问题记录](https://github.com/metasequoiaime/msime/issues)（open issues / PR，含学习同步、词库自改进闭环、
  TSF 前端防闪等方向），为本项目的功能规划与实现提供了宝贵参考，整理见
  [水杉开放需求清单](doc/shuishan-msime-requirements.md)（覆盖 [metasequoiaime 组织](https://github.com/orgs/metasequoiaime/repositories) 全部 20 个仓库的主仓与 Windows 产品仓 issues/PR）。水杉输入法以 GPL-3.0 开源，
  面向 Android、iOS、macOS、Linux、Windows 与 HarmonyOS 六平台的原生宿主 + 共享 Rust 输入运行时，值得一读。
