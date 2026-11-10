//! 注入自测。需先安装驱动并启动服务。
//!
//! 默认按阶段跑：移动光标 → 相对移动 → 滚轮 → 键盘按键 → 鼠标按钮。
//! 运行前先移开鼠标、把焦点放在一个空窗口里。
//!
//! `selftest.exe <stage>` 只跑某个阶段，用于逐级排除问题：
//!   status   只读状态
//!   center   只发一条绝对移动
//!   moves    三条绝对移动
//!   rel      20ms 相对移动循环
//!   input    滚轮/按钮/键盘
//!   media    音量/静音
//! 每个阶段前后都会打印驱动计数器，dropped 增长说明提交被丢弃。

use std::thread::sleep;
use std::time::Duration;

use vhid_input::{
    init_dpi_awareness, virtual_rect, MediaKey, MouseButton, Status, VhidInput,
};

fn dump_status(label: &str, v: &VhidInput) -> Result<(), Box<dyn std::error::Error>> {
    let st: Status = v.status()?;
    let (proto, pending, dropped, maxp) = (
        st.protocol_version,
        st.pending_reports,
        st.dropped_reports,
        st.max_pending,
    );
    println!("[{label}] protocol={proto} pending={pending} dropped={dropped} max={maxp}");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_dpi_awareness();
    let (w, h) = virtual_rect();
    println!("virtual desktop size: {w}x{h}");

    let stage = std::env::args().nth(1).unwrap_or_else(|| "all".into());

    let mut v = VhidInput::open()?;
    dump_status("open", &v)?;

    if stage == "status" {
        return Ok(());
    }

    if stage == "center" || stage == "all" {
        println!("-> move_to center");
        v.move_to(w / 2, h / 2)?;
        sleep(Duration::from_millis(500));
        dump_status("center", &v)?;
        if stage == "center" {
            return Ok(());
        }
    }

    if stage == "moves" || stage == "all" {
        println!("-> move_to top-left");
        v.move_to(50, 50)?;
        sleep(Duration::from_millis(500));

        println!("-> move_to bottom-right");
        v.move_to(w - 50, h - 50)?;
        sleep(Duration::from_millis(500));
        dump_status("moves", &v)?;
        if stage == "moves" {
            return Ok(());
        }
    }

    if stage == "rel" || stage == "all" {
        println!("-> relative move +100,-50 x4");
        for _ in 0..4 {
            v.move_rel(100, -50)?;
            sleep(Duration::from_millis(20));
        }
        sleep(Duration::from_millis(300));
        dump_status("rel", &v)?;
        if stage == "rel" {
            return Ok(());
        }
    }

    if stage == "input" || stage == "all" {
        println!("-> wheel down 3 notches, up 3 notches");
        v.wheel(-360)?;
        sleep(Duration::from_millis(200));
        v.wheel(360)?;
        sleep(Duration::from_millis(200));

        println!("-> mouse left click");
        v.mouse_press(MouseButton::Left)?;
        sleep(Duration::from_millis(100));

        println!("-> keyboard: type 'a' then space");
        // HID Usage: 'a' = 0x04, space = 0x2C
        v.press(0x04)?;
        sleep(Duration::from_millis(50));
        v.press(0x2C)?;
        sleep(Duration::from_millis(200));

        println!("-> keyboard: Ctrl+A");
        v.key_down(0xE0)?; // Left Ctrl
        v.press(0x04)?; // a
        v.key_up(0xE0)?;
        dump_status("input", &v)?;

        v.release_all()?;
    }

    if stage == "media" || stage == "all" {
        println!("-> media: volume up, down, mute");
        for key in [MediaKey::VolumeUp, MediaKey::VolumeDown, MediaKey::Mute] {
            v.media_press(key, Duration::from_millis(80))?;
            sleep(Duration::from_millis(120));
        }
        dump_status("media", &v)?;
    }

    println!("selftest done");
    Ok(())
}
