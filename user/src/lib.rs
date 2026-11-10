//! 用户态接口：通过 `\\.\VhidInput` 向 vhidinput KMDF 驱动注入键盘/鼠标报告。
//!
//! 支持：
//! * 绝对移动 `move_to(x, y)`（像素坐标，自动归一化到 0..32767）
//! * 相对移动 `move_rel(dx, dy)`
//! * 滚轮 `wheel(delta)`
//! * 键盘 `key_down` / `key_up` / `press`（HID Usage Code）
//! * 5 键鼠标：左、右、中、侧 1、侧 2
//! * 媒体键 `media_press`（音量、静音、上下曲、播放/暂停）
//!
//! 归一化：HID 绝对鼠标的 0..32767 映射到整个虚拟桌面，而多显示器下虚拟
//! 桌面可能从负坐标开始。因此 `move_to` 接收的是虚拟桌面坐标（左上角为
//! `virtual_rect()` 的原点），而不是主显示器坐标。

#![allow(non_snake_case)]

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN,
};

// ---------------------------------------------------------------------------
// 与驱动共享的 ABI
// ---------------------------------------------------------------------------

/// 报告 ID，与 `vhidinput.h` 保持一致。
pub const REPORT_ID_KEYBOARD: u8 = 0x01;
pub const REPORT_ID_MOUSE_REL: u8 = 0x02;
pub const REPORT_ID_MOUSE_ABS: u8 = 0x03;
pub const REPORT_ID_MEDIA: u8 = 0x04;

/// Consumer Control 位序，与描述符中 6 个 1 位字段的顺序一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MediaKey {
    VolumeUp = 0,
    VolumeDown = 1,
    Mute = 2,
    ScanNext = 3,
    ScanPrev = 4,
    PlayPause = 5,
}

/// 绝对坐标逻辑范围上限（HID 描述符中的 Logical Maximum）。
pub const ABS_MAX: i32 = 32767;

const DEVICE_PATH: &[u16] = &[
    b'\\' as u16, b'\\' as u16, b'.' as u16, b'\\' as u16,
    b'V' as u16, b'h' as u16, b'i' as u16, b'd' as u16, b'I' as u16, b'n' as u16,
    b'p' as u16, b'u' as u16, b't' as u16, 0,
];

/// 与驱动 `VHIDINPUT_REPORT` 一一对应（65 字节，packed）。
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RawReport {
    report_id: u8,
    data: [u8; 64],
}

/// 与驱动 `VHIDINPUT_STATUS` 一一对应。
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct Status {
    pub protocol_version: u32,
    pub pending_reports: u32,
    pub dropped_reports: u32,
    pub max_pending: u32,
    /// 按下标对应 report ID 的成功提交总数（下标 0 未用）。
    pub submitted: [u32; 8],
}

const fn ctl_code(device_type: u32, function: u32, method: u32, access: u32) -> u32 {
    (device_type << 16) | (access << 14) | (function << 2) | method
}

const FILE_READ_ACCESS: u32 = 0x0001;
const FILE_WRITE_ACCESS: u32 = 0x0002;
const METHOD_BUFFERED: u32 = 0;

const VHIDINPUT_DEVICE_TYPE: u32 = 0x9F00;
/// CTL_CODE(0x9F00, 0x801, METHOD_BUFFERED, FILE_WRITE_DATA|FILE_READ_DATA)
const IOCTL_SUBMIT_REPORT: u32 = ctl_code(
    VHIDINPUT_DEVICE_TYPE,
    0x801,
    METHOD_BUFFERED,
    FILE_READ_ACCESS | FILE_WRITE_ACCESS,
);
/// CTL_CODE(0x9F00, 0x802, METHOD_BUFFERED, FILE_READ_DATA)
const IOCTL_GET_STATUS: u32 =
    ctl_code(VHIDINPUT_DEVICE_TYPE, 0x802, METHOD_BUFFERED, FILE_READ_ACCESS);

// CreateFileW / GENERIC_*
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

// ---------------------------------------------------------------------------

/// 5 个鼠标按钮，位序与 HID 描述符一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
    Side1 = 3,
    Side2 = 4,
}

#[derive(Debug)]
pub enum Error {
    Open(std::io::Error),
    Io(std::io::Error),
    InvalidReportId(u8),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Open(e) => write!(f, "打开 \\\\.\\VhidInput 失败: {e}"),
            Error::Io(e) => write!(f, "DeviceIoControl 失败: {e}"),
            Error::InvalidReportId(id) => write!(f, "无效的 report id: 0x{id:02x}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Open(e) | Error::Io(e) => e,
            Error::InvalidReportId(id) => std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("无效的 report id: 0x{id:02x}"),
            ),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

/// 打开 VhidInput 设备。
pub struct VhidInput {
    handle: HANDLE,
   // 已按下的按钮状态，避免相对鼠标报告反复清零
    buttons: u8,
    // 已按下的键盘键（HID Usage Code），最多 6 个
    keys: [u8; 6],
    modifiers: u8,
}

impl VhidInput {
    /// 打开设备。需要驱动已安装、服务已启动。
    pub fn open() -> Result<Self> {
        let mut path = [0u16; 32];
        path[..DEVICE_PATH.len()].copy_from_slice(DEVICE_PATH);
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        // CreateFileW here is declared by hand and returns the raw HANDLE
        // value, so `VhidInput::drop` is the only owner of the close. That
        // matters if the signature is ever swapped for the windows-sys one,
        // where a handle-shaped type would close itself on drop.
        let handle = raw as HANDLE;
        if handle == INVALID_HANDLE_VALUE {
            return Err(Error::Open(std::io::Error::last_os_error()));
        }
        Ok(Self {
            handle,
            buttons: 0,
            keys: [0; 6],
            modifiers: 0,
        })
    }

    /// 查询驱动状态。
    pub fn status(&self) -> Result<Status> {
        let mut status = Status::default();
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_GET_STATUS,
                std::ptr::null(),
                0,
                &mut status as *mut _ as *mut _,
                std::mem::size_of::<Status>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        Ok(status)
    }

    fn submit(&self, report: RawReport) -> Result<()> {
        let mut returned: u32 = 0;
        let mut pending: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_SUBMIT_REPORT,
                &report as *const _ as *const _,
                std::mem::size_of::<RawReport>() as u32,
                &mut pending as *mut _ as *mut _,
                std::mem::size_of::<u32>() as u32,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(Error::Io(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    // -- 绝对鼠标 --------------------------------------------------------

    /// 将光标移动到虚拟桌面坐标 (x, y)（左上角为原点）。
    ///
    /// 单显示器下与屏幕像素坐标相同；多显示器下虚拟桌面可能比主显示器
    /// 大且从负坐标开始，坐标请以 `virtual_rect()` 为基准换算。
    pub fn move_to(&mut self, x: i32, y: i32) -> Result<()> {
        let (vw, vh) = virtual_rect();
        let ax = scale(x, vw);
        let ay = scale(y, vh);
        self.submit_abs(ax, ay)
    }

    /// 以归一化坐标 (0..ABS_MAX) 移动，调用者自行处理缩放。
    pub fn move_to_normalized(&mut self, x: i32, y: i32) -> Result<()> {
        self.submit_abs(x.clamp(0, ABS_MAX), y.clamp(0, ABS_MAX))
    }

    fn submit_abs(&mut self, x: i32, y: i32) -> Result<()> {
        let mut r = RawReport {
            report_id: REPORT_ID_MOUSE_ABS,
            data: [0; 64],
        };
        r.data[0] = self.buttons;
        r.data[1..3].copy_from_slice(&(x as u16).to_le_bytes());
        r.data[3..5].copy_from_slice(&(y as u16).to_le_bytes());
        // wheel 为 0
        self.submit(r)
    }

    // -- 相对鼠标 --------------------------------------------------------

    /// 相对移动，`dx`/`dy` 为 HID 相对值（-32767..32767）。
    pub fn move_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut r = RawReport {
            report_id: REPORT_ID_MOUSE_REL,
            data: [0; 64],
        };
        r.data[0] = self.buttons;
        r.data[1..3].copy_from_slice(&(dx as i16).to_le_bytes());
        r.data[3..5].copy_from_slice(&(dy as i16).to_le_bytes());
        self.submit(r)
    }

    /// 滚轮，正值向上滚动。按 HID 约定 1 格 = 120。
    pub fn wheel(&mut self, delta: i32) -> Result<()> {
        let notches = (delta / 120).clamp(-127, 127) as i8;
        let mut r = RawReport {
            report_id: REPORT_ID_MOUSE_REL,
            data: [0; 64],
        };
        r.data[0] = self.buttons;
        r.data[5] = notches as u8;
        self.submit(r)
    }

    // -- 鼠标按钮 --------------------------------------------------------

    pub fn mouse_down(&mut self, button: MouseButton) -> Result<()> {
        self.buttons |= 1 << (button as u8);
        self.send_button_report()
    }

    pub fn mouse_up(&mut self, button: MouseButton) -> Result<()> {
        self.buttons &= !(1 << (button as u8));
        self.send_button_report()
    }

    pub fn mouse_press(&mut self, button: MouseButton) -> Result<()> {
        self.mouse_down(button)?;
        self.mouse_up(button)
    }

    fn send_button_report(&mut self) -> Result<()> {
        let mut r = RawReport {
            report_id: REPORT_ID_MOUSE_REL,
            data: [0; 64],
        };
        r.data[0] = self.buttons;
        // 交错的设备（用相对鼠标）发按钮，用绝对鼠标发也一样；这里用相对
        // 鼠标，因为大多数 OS 对按钮的刷新最及时。
        self.submit(r)
    }

    // -- 键盘 ------------------------------------------------------------

    /// 按下键（HID Usage Code，键盘页 0x07）。
    pub fn key_down(&mut self, usage: u8) -> Result<()> {
        if (0xE0..=0xE7).contains(&usage) {
            self.modifiers |= 1 << (usage - 0xE0);
        } else if !self.keys.contains(&usage) {
            // Find the slot first: failing after setting a modifier bit would
            // leave the host with an unreported half-pressed state.
            match self.keys.iter_mut().find(|k| **k == 0) {
                Some(slot) => *slot = usage,
                None => {
                    return Err(Error::Io(std::io::Error::other(
                        "键位已满（最多 6 个非修饰键同时按下）",
                    )))
                }
            }
        }
        self.send_keyboard_report()
    }

    /// 松开键。
    pub fn key_up(&mut self, usage: u8) -> Result<()> {
        if (0xE0..=0xE7).contains(&usage) {
            self.modifiers &= !(1 << (usage - 0xE0));
        } else if let Some(pos) = self.keys.iter().position(|&k| k == usage) {
            self.keys[pos] = 0;
        }
        self.send_keyboard_report()
    }

    /// 按下并松开。
    pub fn press(&mut self, usage: u8) -> Result<()> {
        self.key_down(usage)?;
        self.key_up(usage)
    }

    fn send_keyboard_report(&self) -> Result<()> {
        let mut r = RawReport {
            report_id: REPORT_ID_KEYBOARD,
            data: [0; 64],
        };
        r.data[0] = self.modifiers;
        r.data[1] = 0; // reserved
        r.data[2..8].copy_from_slice(&self.keys);
        self.submit(r)
    }

    // -- 媒体键 ----------------------------------------------------------

    /// 按下媒体键（Consumer Control），需配对调用 `media_release`。
    pub fn media_down(&self, key: MediaKey) -> Result<()> {
        let mut r = RawReport {
            report_id: REPORT_ID_MEDIA,
            data: [0; 64],
        };
        r.data[0] = 1 << (key as u8);
        self.submit(r)
    }

    /// 释放全部媒体键：所有位清零，即“没有键按下”。
    pub fn media_release(&self) -> Result<()> {
        let r = RawReport {
            report_id: REPORT_ID_MEDIA,
            data: [0; 64],
        };
        self.submit(r)
    }

    /// 按一下媒体键（按下后保持 `hold` 再释放）。
    pub fn media_press(&self, key: MediaKey, hold: std::time::Duration) -> Result<()> {
        self.media_down(key)?;
        std::thread::sleep(hold);
        self.media_release()
    }

    /// 释放所有按键与按钮（避免程序退出后输入卡住）。
    pub fn release_all(&mut self) -> Result<()> {
        self.buttons = 0;
        self.modifiers = 0;
        self.keys = [0; 6];
        self.send_button_report()?;
        self.media_release()?;
        self.send_keyboard_report()
    }
}

impl Drop for VhidInput {
    fn drop(&mut self) {
        // 尽力释放；设备已被移除时这里必然失败，但句柄仍要关闭。
        if let Err(e) = self.release_all() {
            eprintln!("vhid-input: 退出时释放按键失败: {e}");
        }
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

// ---------------------------------------------------------------------------
// 坐标归一化
// ---------------------------------------------------------------------------

/// 虚拟桌面的像素尺寸 (宽, 高)。
///
/// HID 绝对鼠标的整个逻辑范围对应整个虚拟桌面，所以这里用
/// `SM_CXVIRTUALSCREEN` / `SM_CYVIRTUALSCREEN`，而不是主显示器的尺寸。
/// 这两个指标是物理像素，与进程 DPI 感知设置无关。
pub fn virtual_rect() -> (i32, i32) {
    unsafe {
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w > 0 && h > 0 {
            (w, h)
        } else {
            (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
        }
    }
}

/// 将虚拟桌面内的偏移 px 映射到 0..ABS_MAX。
fn scale(v: i32, dim: i32) -> i32 {
    if dim <= 1 {
        return 0;
    }
    // 取像素中心再缩放，避免把整列像素压到同一个逻辑值上。
    let v = v as i64 * 2 + 1;
    let d = dim as i64 * 2;
    ((v * (ABS_MAX as i64 + 1)) / d).clamp(0, ABS_MAX as i64) as i32
}

/// 调用一次，将当前进程设为 Per-Monitor DPI Aware V2，保证坐标是物理像素。
pub fn init_dpi_awareness() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 简写：把 `key_down`/`key_up` 的用法固定到 `press`。
pub fn press_key(v: &mut VhidInput, usage: u8) -> Result<()> {
    v.press(usage)
}

// ---------------------------------------------------------------------------
// 外部 Win32 声明（CreateFileW 在 windows-sys 0.60 中位于 Foundation，但
// 这里直接声明以避免额外 feature 依赖差异）
// ---------------------------------------------------------------------------

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut std::ffi::c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: HANDLE,
    ) -> HANDLE;
}
