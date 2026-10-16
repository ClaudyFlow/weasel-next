# 重启后要验证的事（2026-10-13 安装后）

安装已完成，84 个文件因被占用排在 `C:\Program Files\Weasel-Next\files.pending.lst`，
重启后由系统在启动早期替换。重启后按顺序确认：

1. 替换是否生效
   - `C:\Program Files\Weasel-Next\Weasel-Next-Server.exe` 时间戳应为 10-13 14:16 之后；
   - `rime.dll` 应为新构建（若已重建 librime 则是当天时间，否则仍是 10-10 22:42）；
   - `weasel_tip.dll` 应为 10-13。

2. 主题是否加载
   - 打字后看 `%LOCALAPPDATA%\Rime-Next\Logs\renderer.<日期>.log`，应出现
     `using renderer theme official`；
   - 首帧绘制时还应出现 `皮肤已加载: <路径>\Rime-Next\theme\default.theme.toml · 配色 aqua（碧水／Aqua）· 底色 #eceeee · 文字 #000000`。

3. 候选窗外观
   - 底色应为 #eceeee（官方 aqua 的 back_color），文字 #000000，选中块 #fa3a0a；
   - 圆角之外应为真正透明（已换成 D3D11 + DXGI 组合交换链 + DirectComposition，逐像素 alpha 交给 DWM）。

4. 如果主题没加载
   - 先看 renderer 日志里的 `theme notice`；
   - 再确认 `C:\Program Files\Weasel-Next\weasel.toml` 里 `theme` 是 `official`；
   - 确认 `weasel_theme_official.dll` 在程序文件夹根目录。

未做的两件事（与本次安装无关）：
- librime 重建（`.theme.toml → theme/` 规则要重建 rime.dll 才生效，上次卡在 leveldb）；
- 用户目录 `%APPDATA%\Rime-Next` 已被上次安装清空，语言模型（约 452 MB）与自定义配置需要重新获取或重建。