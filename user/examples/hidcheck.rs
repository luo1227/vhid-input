//! 列出系统里所有虚拟 HID（VHF）设备及其子集合，方便确认驱动是否真的枚举
//! 出键盘/鼠标并能投递输入。

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
    SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, DIGCF_ALLCLASSES,
    DIGCF_PRESENT, SP_DEVINFO_DATA, SPDRP_DEVICEDESC, SPDRP_FRIENDLYNAME,
    SPDRP_HARDWAREID, SPDRP_MFG,
};

unsafe fn get_str(h: isize, dev: &SP_DEVINFO_DATA, prop: u32) -> Option<String> {
    let mut buf = [0u8; 1024];
    let mut need = 0u32;
    let ok = SetupDiGetDeviceRegistryPropertyW(
        h,
        dev,
        prop,
        std::ptr::null_mut(),
        buf.as_mut_ptr(),
        buf.len() as u32,
        &mut need,
    );
    if ok == 0 {
        return None;
    }
    let w: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&c| c != 0)
        .collect();
    if w.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&w))
}

fn main() {
    unsafe {
        // Every present device; filter for VHF below.
        let all = SetupDiGetClassDevsW(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut(), (DIGCF_PRESENT | DIGCF_ALLCLASSES) as u32);
        if all == usize::MAX as isize {
            println!("SetupDiGetClassDevs failed");
            return;
        }
        let mut dev: SP_DEVINFO_DATA = std::mem::zeroed();
        dev.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
        let mut i = 0u32;
        let mut found = 0;
        while SetupDiEnumDeviceInfo(all, i, &mut dev) != 0 {
            i += 1;
            let mut id = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(all, &dev, id.as_mut_ptr(), id.len() as u32, std::ptr::null_mut()) == 0 {
                continue;
            }
            let n = id.iter().position(|&c| c == 0).unwrap_or(0);
            let inst = String::from_utf16_lossy(&id[..n]);
            if !inst.to_uppercase().contains("VHF") {
                continue;
            }
            found += 1;
            let hw = get_str(all, &dev, SPDRP_HARDWAREID).unwrap_or_default();
            let name = get_str(all, &dev, SPDRP_FRIENDLYNAME)
                .or_else(|| get_str(all, &dev, SPDRP_DEVICEDESC))
                .unwrap_or_default();
            let mfg = get_str(all, &dev, SPDRP_MFG).unwrap_or_default();
            println!("{inst}");
            println!("    name: {name}");
            println!("    mfg : {mfg}");
            println!("    hwid: {}", hw.replace('\0', " | "));
        }
        SetupDiDestroyDeviceInfoList(all);
        println!("{found} VHF device node(s) present");
    }
}
