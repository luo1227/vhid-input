//! 注册 Raw Input 并打印收到的原始鼠标/键盘事件。
//!
//! 用来判断 hidclass 是否把我们的 VHF 报告分派成了 Raw Input 事件：
//! Raw Input 走 hidclass → 输入栈，比 mouhid 更靠近源头。如果这里能收到
//! 我们注入的字节，说明分派正确，问题在 mouhid/会话层；收不到则在分派。
//!
//! 用法：rawinput [秒数]，默认 8 秒。期间在另一个窗口注入报告。

use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_LBUTTON,
};
use windows_sys::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_INPUTSINK};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW,
    RegisterClassW, TranslateMessage, MSG, PM_REMOVE, WS_OVERLAPPEDWINDOW,
    WNDCLASSW,
};
use windows_sys::Win32::UI::Input::RegisterRawInputDevices;

#[repr(C)]
#[derive(Clone, Copy)]
struct RAWINPUTHEADER {
    dwType: u32,
    dwSize: u32,
    hDevice: isize,
    wParam: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MOUSEINPUT {
    dx: i32,
    dy: i32,
    mouseData: u32,
    dwFlags: u32,
    time: u32,
    dwExtraInfo: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RAWMOUSE {
    usFlags: u16,
    _pad: u16,
    u: MouseUnion,
    ulRawButtons: u32,
    lLastX: i32,
    lLastY: i32,
    ulExtraInformation: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
union MouseUnion {
    ulButtons: u32,
    buttons: MOUSEINPUT,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RAWKEYBOARD {
    MakeCode: u16,
    Flags: u16,
    Reserved: u16,
    VKey: u16,
    Message: u32,
    ExtraInformation: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
union RawData {
    mouse: RAWMOUSE,
    keyboard: RAWKEYBOARD,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RAWINPUT {
    header: RAWINPUTHEADER,
    data: RawData,
}

#[link(name = "user32")]
extern "system" {
    fn GetRawInputData(
        hRawInput: isize,
        uiCommand: u32,
        pData: *mut std::ffi::c_void,
        pcbSize: *mut u32,
        cbSizeHeader: u32,
    ) -> u32;
}

const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPEMOUSE: u32 = 0;
const RIM_TYPEKEYBOARD: u32 = 1;

static mut COUNT_MOUSE: u32 = 0;
static mut COUNT_KEY: u32 = 0;

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    const WM_INPUT: u32 = 0x00FF;
    if msg == WM_INPUT {
        let mut size: u32 = 0;
        // 先问结构大小。
        GetRawInputData(
            lparam as isize,
            RID_INPUT,
            null_mut(),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
        if size > 0 {
            let mut buf = vec![0u8; size as usize];
            let got = GetRawInputData(
                lparam as isize,
                RID_INPUT,
                buf.as_mut_ptr() as *mut _,
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            );
            if got > 0 {
                let ri = &*(buf.as_ptr() as *const RAWINPUT);
                if ri.header.dwType == RIM_TYPEMOUSE {
                    let m = ri.data.mouse;
                    let buttons = m.u.ulButtons;
                    println!(
                        "mouse dev=h{:X} flags=0x{:04X} buttons=0x{:08X} last=({},{})",
                        ri.header.hDevice, m.usFlags, buttons, m.lLastX, m.lLastY
                    );
                    COUNT_MOUSE += 1;
                } else if ri.header.dwType == RIM_TYPEKEYBOARD {
                    let k = ri.data.keyboard;
                    println!(
                        "key   dev=h{:X} vkey=0x{:02X} make=0x{:02X} flags=0x{:04X} msg=0x{:X}",
                        ri.header.hDevice, k.VKey, k.MakeCode, k.Flags, k.Message
                    );
                    COUNT_KEY += 1;
                }
            }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);

    unsafe {
        let hinst = GetModuleHandleW(null_mut());
        let class_name = wide("RawInputProbe");
        let mut wc: WNDCLASSW = zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinst;
        wc.lpszClassName = class_name.as_ptr();
        RegisterClassW(&wc);

        let title = wide("probe");
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst,
            null_mut(),
        );
        if hwnd == null_mut() {
            println!("CreateWindowEx failed: {}", std::io::Error::last_os_error());
            return;
        }

        // 注册鼠标（Generic Desktop 0x01 / usage 0x02）和键盘（0x01 / 0x06）。
        // RIDEV_INPUTSINK 允许后台接收。
        let mut devs = [
            RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x02,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x06,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: 0x0C,
                usUsage: 0x01,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
        ];
        if RegisterRawInputDevices(
            devs.as_mut_ptr(),
            devs.len() as u32,
            size_of::<RAWINPUTDEVICE>() as u32,
        ) == 0
        {
            println!(
                "RegisterRawInputDevices failed: {}",
                std::io::Error::last_os_error()
            );
            return;
        }
        // 清一次按键状态，保证提示准确。
        let _ = GetKeyState(VK_LBUTTON as i32);

        println!("listening raw input for {secs}s ...");
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut msg: MSG = zeroed();
        while Instant::now() < deadline {
            while PeekMessageW(&mut msg, hwnd, 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        println!(
            "done: mouse={} key={}",
            COUNT_MOUSE, COUNT_KEY
        );
    }
}

/// 宽字符串字面量的简化 helper。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
