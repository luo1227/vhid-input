//! 检查虚拟键盘是否真的工作在系统里：用 AttachThreadInput 把本线程附加到
//! 前台线程的输入队列，然后读 GetKeyState(VK_CONTROL)。注入 Ctrl 按下后
//! 这里应为负数。用来判断注入报告是否被系统消费。

use std::thread::sleep;
use std::time::Duration;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId,
};

// AttachThreadInput is not covered by the windows-sys features in use.
extern "system" {
    fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: i32) -> i32;
}

fn main() {
    let mut state = String::new();
    let reader = std::io::stdin();
    let mut v = vhid_input::VhidInput::open().expect("open");
    println!("attach ctrl down ctrl up state each line; empty line quits");
    while reader.read_line(&mut state).is_ok() && state.trim() != ""{
        match state.trim() {
            "attach" => unsafe {
                let fg = GetForegroundWindow();
                let mut pid = 0u32;
                let tid = GetWindowThreadProcessId(fg, &mut pid);
                let me = windows_sys::Win32::System::Threading::GetCurrentThreadId();
                let ok = AttachThreadInput(me, tid, 1);
                println!("foreground hwnd={fg:?} pid={pid} tid={tid} me={me} attach={ok}");
            },
            "down" => {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL;
                let _ = v.key_down(0xE0);
                sleep(Duration::from_millis(100));
                let s = unsafe { GetKeyState(VK_CONTROL as i32) };
                println!("ctrl down -> GetKeyState={s}");
            }
            "up" => {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL;
                let _ = v.key_up(0xE0);
                sleep(Duration::from_millis(100));
                let s = unsafe { GetKeyState(VK_CONTROL as i32) };
                println!("ctrl up -> GetKeyState={s}");
            }
            other => println!("unknown: {other}"),
        }
        state.clear();
    }
}
