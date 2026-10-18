//! 应用菜单相关功能

pub const BROKER_WINDOW_CLASS: &str = "weasel-next-broker";
pub const DEPLOY: u32 = 1001;
pub const EXIT: u32 = 1002;
pub const USER_DIRECTORY: u32 = 1003;
pub const PROGRAM_DIRECTORY: u32 = 1004;
pub const LOG_DIRECTORY: u32 = 1005;
pub const HELP: u32 = 1006;
pub const FORUM: u32 = 1007;
pub const RESTART: u32 = 1008;
pub const ABOUT: u32 = 1009;
pub const DIAGNOSTICS: u32 = 1010;
pub const SETTINGS: u32 = 1011;
pub const CHECK_UPDATES: u32 = 1012;
pub const STATISTICS: u32 = 1013;
pub const SWITCHER: u32 = 1014;
pub const STYLE: u32 = 1015;

pub const ITEMS: &[(u32, &str)] = &[
    (SETTINGS, "设置 (&S)"),
    // 「输入统计」「界面风格」「方案选单」都收进设置主界面（用户 2026-10-18：
    // 这三个是设置里的功能，不要放到右键菜单）。STATISTICS / STYLE / SWITCHER 三个
    // 命令 id 仍然保留 —— 语言栏/旧客户端可能还在发它们，`menu_actions` 照旧能处理，
    // 只是菜单里不再列出。
    (USER_DIRECTORY, "用户文件夹 (&U)"),
    (PROGRAM_DIRECTORY, "程序文件夹 (&P)"),
    (LOG_DIRECTORY, "日志文件夹 (&L)"),
    // 分隔线
    (0, ""),
    (HELP, "帮助 (&H)"),
    (FORUM, "论坛 (&F)"),
    (ABOUT, "关于小狼毫Next (&A)"),
    (CHECK_UPDATES, "检查更新 (&C)"),
    // 分隔线
    (0, ""),
    (DEPLOY, "重新部署Rime (&D)"),
    (RESTART, "重启算法服务 (&R)"),
    (EXIT, "退出 (&X)"),
];

/// 按显示顺序生成菜单项，并可在“关于”项后插入诊断命令。
/// `diagnostics` 为 `false` 时不暴露诊断入口。
pub fn items(diagnostics: bool) -> impl Iterator<Item = (u32, &'static str)> {
    ITEMS.iter().copied().flat_map(move |item| {
        [
            Some(item),
            (diagnostics && item.0 == ABOUT).then_some((DIAGNOSTICS, "诊断信息… (&I)")),
        ]
        .into_iter()
        .flatten()
    })
}

/// 判断 ID 是否对应基础菜单中的有效命令。
pub fn is_command(id: u32) -> bool {
    id != 0 && ITEMS.iter().any(|(command, _)| *command == id)
}
