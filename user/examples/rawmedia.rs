//! 直接发一条媒体键报告（report ID 4），用来验证 COL04（Consumer Control）
//! 的投递与读取链路。data[0] 的位对应 Volume Inc/Dec/Mute/ScanNext/ScanPrev/Play。
//!
//! 用法：rawmedia <bitmask 0..63> [hold_ms]

use std::fs::OpenOptions;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::System::IO::DeviceIoControl;

#[repr(C, packed)]
struct Raw {
    id: u8,
    data: [u8; 64],
}

const IOCTL_SUBMIT: u32 = (0x9F00u32 << 16) | (3u32 << 14) | (0x801u32 << 2);

fn submit(f: &std::fs::File, r: &Raw) {
    use std::os::windows::io::AsRawHandle as _;
    let h = f.as_raw_handle() as *mut std::ffi::c_void;
    let mut ret: u32 = 0;
    let mut pending: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_SUBMIT,
            r as *const _ as *const _,
            std::mem::size_of::<Raw>() as u32,
            &mut pending as *mut _ as *mut _,
            4,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        println!("submit failed pending={pending} err={}", std::io::Error::last_os_error());
    } else {
        println!("submit ok pending={pending}");
    }
}

fn main() {
    let mask: u8 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let hold_ms: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);

    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open("\\\\.\\VhidInput")
        .expect("open the VhidInput device");
    let _ = f.as_raw_handle();

    let mut down = Raw { id: 4, data: [0; 64] };
    down.data[0] = mask;
    println!("media down mask=0x{mask:02X}");
    submit(&f, &down);

    std::thread::sleep(std::time::Duration::from_millis(hold_ms));

    let up = Raw { id: 4, data: [0; 64] };
    println!("media up");
    submit(&f, &up);
}
