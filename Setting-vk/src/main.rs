//! Vulkan 版设置外壳(weasel-setting-vk):
//! 设置界面骨架(ui)+ GDI 画布(canvas),真数据统计。
//! 候选窗/背景模糊/后端探测已拆到 ../Renderer-VK(2026-11-01,两线分离)。
//!
//! 用法:`Weasel-Next-Setting-VK.exe ui`(默认即 ui)

mod canvas;
mod ui;

use ash::Entry;

const WM_DESTROY: u32 = 0x0002;
const WM_KEYDOWN: u32 = 0x0100;
const VK_ESCAPE: usize = 0x1B;

pub(crate) unsafe extern "system" fn wnd_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe {
        match msg {
            WM_DESTROY => {
                windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                windows::Win32::Foundation::LRESULT(0)
            }
            WM_KEYDOWN if wp.0 == VK_ESCAPE => {
                windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
                windows::Win32::Foundation::LRESULT(0)
            }
            _ => windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).unwrap_or_default();
    unsafe {
        let entry = Entry::load()?;
        match mode.as_str() {
            // 候选窗已迁 Renderer-VK:这里只管设置界面
            "cand" | "probe" | "window" | "dump" => {
                println!("该模式已迁至 Weasel-Next-Renderer-VK.exe");
            }
            _ => ui::run(&entry)?,
        }
    }
    Ok(())
}
