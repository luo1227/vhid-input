//! 最小的绝对移动注入，直接走 DeviceIoControl，不依赖库的坐标换算和状态。
//! 用来把问题切成“驱动链路”和“用户态库”两半。
//!
//! 用法：rawmove <absx 0..32767> <absy 0..32767>

use std::fs::OpenOptions;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::System::IO::DeviceIoControl;

#[repr(C, packed)]
struct Raw {
    id: u8,
    data: [u8; 64],
}

// CTL_CODE(0x9F00, 0x801, METHOD_BUFFERED, FILE_READ_DATA|FILE_WRITE_DATA)
const IOCTL_SUBMIT: u32 = (0x9F00u32 << 16) | (3u32 << 14) | (0x801u32 << 2);

fn main() {
    let args: Vec<i32> = std::env::args()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    let x = args.first().copied().unwrap_or(16384);
    let y = args.get(1).copied().unwrap_or(16384);

    // Optional third argument selects the report ID: 3 = absolute (default),
    // 2 = relative. For relative, the same two values are signed dx/dy.
    let id: u8 = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let mut r = Raw { id, data: [0; 64] };
    if id == 2 {
        r.data[1..3].copy_from_slice(&(x as i16).to_le_bytes());
        r.data[3..5].copy_from_slice(&(y as i16).to_le_bytes());
    } else {
        r.data[1..3].copy_from_slice(&(x as u16).to_le_bytes());
        r.data[3..5].copy_from_slice(&(y as u16).to_le_bytes());
    }

    println!("ctl=0x{IOCTL_SUBMIT:08X} x={x} y={y}");
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open("\\\\.\\VhidInput")
        .expect("open the VhidInput device");
    let h = f.as_raw_handle() as *mut std::ffi::c_void;
    let mut ret: u32 = 0;
    let mut pending: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_SUBMIT,
            &r as *const _ as *const _,
            std::mem::size_of::<Raw>() as u32,
            &mut pending as *mut _ as *mut _,
            4,
            &mut ret,
            std::ptr::null_mut(),
        )
    };
    // last_os_error() 只在失败时有意义：成功时上一轮的 GetLastError 会被
    // 原样带出来，打印它只会误导。
    if ok == 0 {
        println!(
            "submit failed pending={pending} err={}",
            std::io::Error::last_os_error()
        );
    } else {
        println!("submit ok pending={pending}");
    }
}
