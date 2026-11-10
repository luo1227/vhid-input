//! 打印当前光标位置，用于在外部确认注入是否真的移动了光标。
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

fn main() {
    let mut p = POINT { x: 0, y: 0 };
    let ok = unsafe { GetCursorPos(&mut p) };
    println!("ok={ok} x={} y={}", p.x, p.y);
}
