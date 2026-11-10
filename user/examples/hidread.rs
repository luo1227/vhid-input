//! 打开本驱动虚拟鼠标集合的 HID 设备路径并读取输入报告。
//!
//! 用来判断“驱动提交报告返回成功、但光标不动”到底断在哪一层：如果这里能
//! 读出我们注入的字节，说明 VHF 到 HID 栈的投递是通的，问题在 mouhid 及以上；
//! 如果读不到，问题在 VhfReadReportSubmit 之前。
//!
//! 用法：hidread [秒数]，默认 6 秒。期间在另一个窗口注入��告。

use std::fs::{File, OpenOptions};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Storage::FileSystem::ReadFile;

const HID_GUID: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x4d1e55b2,
    data2: 0xf16f,
    data3: 0x11cf,
    data4: [0x88, 0xcb, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30],
};

#[link(name = "hid")]
extern "system" {
    fn HidD_GetAttributes(
        hid_device: *mut std::ffi::c_void,
        attributes: *mut HIDD_ATTRIBUTES,
    ) -> i32;
    fn HidD_GetPreparsedData(hid_device: *mut std::ffi::c_void, data: *mut isize) -> i32;
    fn HidD_FreePreparsedData(data: isize) -> i32;
    fn HidP_GetCaps(preparsed: isize, caps: *mut HIDP_CAPS) -> i32;
}

#[repr(C)]
#[derive(Default)]
struct HIDD_ATTRIBUTES {
    size: u32,
    vendor_id: u16,
    product_id: u16,
    version_number: u16,
}

#[repr(C)]
#[derive(Default)]
struct HIDP_CAPS {
    usage: u16,
    usage_page: u16,
    input_report_byte_length: u16,
    output_report_byte_length: u16,
    feature_report_byte_length: u16,
    reserved: [u16; 17],
    number_link_collection_nodes: u16,
    number_input_button_caps: u16,
    number_input_value_caps: u16,
    number_output_button_caps: u16,
    number_output_value_caps: u16,
    number_feature_button_caps: u16,
    number_feature_value_caps: u16,
}

fn dump_caps(f: &File) {
    let mut pp: isize = 0;
    if unsafe { HidD_GetPreparsedData(f.as_raw_handle() as _, &mut pp) } == 0 {
        println!("HidD_GetPreparsedData failed");
        return;
    }
    let mut caps = HIDP_CAPS::default();
    let st = unsafe { HidP_GetCaps(pp, &mut caps) };
    println!(
        "caps: status=0x{st:08X} usage_page=0x{:04X} usage=0x{:04X} in={} out={} feat={} buttons={}",
        caps.usage_page,
        caps.usage,
        caps.input_report_byte_length,
        caps.output_report_byte_length,
        caps.feature_report_byte_length,
        caps.number_input_button_caps,
    );
    unsafe {
        HidD_FreePreparsedData(pp);
    }
}

fn find_our_device() -> Option<String> {
    unsafe {
        let h = SetupDiGetClassDevsW(
            &HID_GUID,
            std::ptr::null(),
            std::ptr::null_mut(),
            (DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) as u32,
        );
        if h == -1 {
            return None;
        }
        let mut iface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
        iface.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

        let mut found = None;
        let mut i = 0u32;
        while SetupDiEnumDeviceInterfaces(
            h,
            std::ptr::null_mut(),
            &HID_GUID,
            i,
            &mut iface,
        ) != 0
        {
            i += 1;
            // Ask for the size, then read the detail with the variable-size path.
            let mut need = 0u32;
            SetupDiGetDeviceInterfaceDetailW(
                h,
                &iface,
                std::ptr::null_mut(),
                0,
                &mut need,
                std::ptr::null_mut(),
            );
            if need == 0 {
                continue;
            }
            let mut buf = vec![0u8; need as usize];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize =
                std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(h, &iface, detail, need, &mut need, std::ptr::null_mut())
                == 0
            {
                continue;
            }
            let path_ptr = (*detail).DevicePath.as_ptr();
            let mut len = 0usize;
            while *path_ptr.add(len) != 0 {
                len += 1;
            }
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(path_ptr, len));

            // Our driver instance is ROOT\SYSTEM\0005, whose VHF children show
            // up as HID#hid_device_system_vhf&colNN#2&<instance>#... The other
            // VHF node (2&cbcac31) belongs to another vendor, so match on the
            // instance token rather than the VID, which the path omits.
            let upper = path.to_uppercase();
            if std::env::var_os("HIDREAD_ALL").is_some() {
                println!("iface: {path}");
            }
            let want = std::env::var("HIDREAD_MATCH")
                .unwrap_or_else(|_| "2&BC48C46".to_string())
                .to_uppercase();
            if !upper.contains(&want) {
                continue;
            }

            match OpenOptions::new().read(true).open(&path) {
                Ok(f) => {
                    let mut attrs = HIDD_ATTRIBUTES {
                        size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
                        ..Default::default()
                    };
                    if HidD_GetAttributes(f.as_raw_handle() as _, &mut attrs) != 0 {
                        println!(
                            "candidate {path}\n    vid=0x{:04x} pid=0x{:04x}",
                            attrs.vendor_id, attrs.product_id
                        );
                        if found.is_none() {
                            found = Some(path.clone());
                        }
                    } else {
                        println!("open ok but HidD_GetAttributes failed: {}",
                                 std::io::Error::last_os_error());
                    }
                }
                Err(e) => println!("open {path} failed: {e}"),
            }
        }
        SetupDiDestroyDeviceInfoList(h);
        found
    }
}

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(6);

    let what: u16 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1); // 1 = the address path of the mouse collection

    let path = match find_our_device() {
        Some(p) => p,
        None => {
            println!("no VID_1209 HID collection found");
            return;
        }
    };
    println!("watching {path} for {secs}s");

    let f: File = match OpenOptions::new().read(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            println!("open failed: {e}");
            return;
        }
    };
    dump_caps(&f);

    // HID input reports are read through ReadFile; the first byte is the
    // report ID for devices that use report IDs.
    let deadline = Instant::now() + Duration::from_secs(secs);
    let mut buf = [0u8; 64];
    let mut total = 0;
    while Instant::now() < deadline {
        let mut got = 0u32;
        let ok = unsafe {
            ReadFile(
                f.as_raw_handle() as _,
                buf.as_mut_ptr() as _,
                buf.len() as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // Non-overlapped handle: the read blocks until a report arrives.
            println!("read failed: {}", std::io::Error::last_os_error());
            break;
        }
        total += 1;
        println!("report: {:02X?}", &buf[..got as usize]);
    }
    println!("{total} report(s) read");
    let _ = what;
}
