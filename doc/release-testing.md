# Weasel-Next 0.1.8 x64 测试包

发给测试者之前请连安装包一起发出。

## 包信息

    文件名   Weasel-Next-0.1.8-x64-setup.exe
    大小     23.12 MB
    构建时间 2026-10-15 01:29:18
    SHA256   201353083ED2AB8765B6218D9A12D75CD5D8056414FDB4988AC854C9A7F44FCD

## 安装

需要 **Windows 10 1809 或更高、x64、管理员权限**。

包未做代码签名，双击时 Windows SmartScreen 会拦一次：
点「更多信息」→「仍要运行」即可。

不需要预先安装 VC++ 运行库——MinGW 运行时已随包附带。

## 建议的测试顺序

1. **安装**：装完检查 `C:\Program Files\Weasel-Next\` 是否存在，
   里面应有四个 exe、rime.dll、weasel_tip.dll、weasel_theme_official.dll、
   六个 lib*.dll、Uninstall.exe，以及 `Rime-Next\` 数据目录。
2. **输入法出现**：任务栏输入法列表里应出现「小狼毫Next」。
3. **打字**（重点）：切换到小狼毫Next，随便打几个字母，
   应能出候选窗并能上屏。候选窗的配色应跟随皮肤设置。
4. **设置程序**：在 `C:\Program Files\Weasel-Next\` 下执行

       Weasel-Next-Setting.exe /style      界面风格设定
       Weasel-Next-Setting.exe /switcher   方案选单设定
       Weasel-Next-Setting.exe /dict       用户词典管理
       Weasel-Next-Setting.exe             不带参数进主设置界面

   `/style` 里下拉框应列出 36 套配色（碧水／Aqua、安卓／Android、青天／Azure…），
   选一套后立刻生效，改动写入 `%APPDATA%\Rime-Next\weasel.custom.yaml`。
5. **卸载**：控制面板卸载后程序目录与开始菜单项应清干净。

## 已知未完成

* `/switcher` 与 `/dict` 只显示官方对话框，内部列表**还没有数据**
  （`/style` 的配色切换是完整可用的）。
* 没有自动更新。
* 包未签名。

## 出问题时请收集

日志在 `%LOCALAPPDATA%\Rime-Next\Logs\`：

    broker.<日期>.log        服务进程
    server.<日期>.log        引擎（含部署与方案加载）
    renderer.<日期>.log      候选窗渲染
    deploy.<日期>.log        部署过程

用户配置与部署产物的位置：

    %APPDATA%\Rime-Next\                用户目录
    %APPDATA%\Rime-Next\build\<方案>\   各方案的编译产物
    %APPDATA%\Rime-Next\weasel.custom.yaml  设置程序写入的补丁

## 干净机器与升级安装的差别

**干净机器**首次安装即装上新版输入法组件，不需要重启。

**装过旧版**的机器上，输入法组件会被资源管理器占用，安装程序无法当场替换，
需要重启一次才生效——这是 Windows 的机制，不是安装包的问题。