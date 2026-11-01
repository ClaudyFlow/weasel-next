use super::*;

#[test]
fn absent_child_shutdown_needs_no_rpc() {
    assert!(shutdown_component(&mut None, "unused-test-pipe".into(), "test").is_ok());
}

/// 引擎/渲染器自己的 broker 连接不算"有人在用"。
#[test]
fn internal_components_do_not_wake_idle_service() {
    assert!(is_internal_client(Some("Weasel-Next-Engine.exe")));
    assert!(is_internal_client(Some("Weasel-Next-Renderer.exe")));
}

/// 宿主进程里的 TIP（任意 exe）、设置界面、以及查不到进程名时都要唤醒 ——
/// 宁可多重启一次，也不能让用户按下第一键时没有输入法。
#[test]
fn external_clients_and_unknown_clients_wake_idle_service() {
    assert!(!is_internal_client(Some("explorer.exe")));
    assert!(!is_internal_client(Some("notepad.exe")));
    assert!(!is_internal_client(Some("Weasel-Next-Setting.exe")));
    assert!(!is_internal_client(None));
}
