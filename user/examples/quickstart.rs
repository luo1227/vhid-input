//! 演示便捷方法：全局单例、拖拽、双击、字面文本、组合键。
//!
//! 运行前提：驱动已装、设备 Running、光标所在的桌面可接收输入。
//! `quickstart.exe [stage]` 只跑某个阶段，方便单独观察：
//!   global   全局单例：click_at / move_rel / release_all
//!   drag     左键从 (200,200) 拖到 (500,400)
//!   dbl      双击左键
//!   text     输入 "Hello, World! 0-9"
//!   chord    Ctrl+A（会全选当前活动窗口内容）
//! 省略参数则依次跑 global → drag → dbl → text。

use std::thread::sleep;
use std::time::Duration;

use vhid_input::{init_dpi_awareness, virtual_rect, MouseButton, VhidInput, MOD_LCTRL};

fn stage_global() -> Result<(), Box<dyn std::error::Error>> {
    // 全局单例：不自己 open()，第一次调用时惰性打开。
    vhid_input::click_at(300, 300, MouseButton::Left)?;
    sleep(Duration::from_millis(100));
    vhid_input::move_rel(50, 0)?;
    sleep(Duration::from_millis(100));
    vhid_input::release_all()?;
    println!("[global] ok（使用了全局单例句柄）");
    Ok(())
}

fn stage_drag(v: &mut VhidInput) -> Result<(), Box<dyn std::error::Error>> {
    v.drag(200, 200, 500, 400)?;
    println!("[drag] (200,200) -> (500,400)");
    Ok(())
}

fn stage_dbl(v: &mut VhidInput) -> Result<(), Box<dyn std::error::Error>> {
    v.move_to(300, 300)?;
    v.double_click(MouseButton::Left)?;
    println!("[dbl] 双击完成");
    Ok(())
}

fn stage_text(v: &mut VhidInput) -> Result<(), Box<dyn std::error::Error>> {
    // 只支持 US ASCII；中文会返回 UnsupportedChar。
    v.key_text("Hello, World! 0-9")?;
    println!("[text] 输入完成");
    Ok(())
}

fn stage_chord(v: &mut VhidInput) -> Result<(), Box<dyn std::error::Error>> {
    v.chord(&[MOD_LCTRL], 0x04)?; // Ctrl+A
    println!("[chord] Ctrl+A 完成");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_dpi_awareness();
    let (w, h) = virtual_rect();
    println!("virtual desktop: {w}x{h}");

    let stage = std::env::args().nth(1).unwrap_or_default();
    let run = |name: &str, f: &mut dyn FnMut() -> Result<(), Box<dyn std::error::Error>>| {
        if stage.is_empty() || stage == name {
            f()?;
            sleep(Duration::from_millis(300));
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    };

    // global 阶段只用单例，不需要实例；其余阶段共用一个实例。
    run("global", &mut || stage_global())?;
    if stage.is_empty() || matches!(stage.as_str(), "drag" | "dbl" | "text" | "chord") {
        let mut v = VhidInput::open()?;
        run("drag", &mut || stage_drag(&mut v))?;
        run("dbl", &mut || stage_dbl(&mut v))?;
        run("text", &mut || stage_text(&mut v))?;
        run("chord", &mut || stage_chord(&mut v))?;
    }
    println!("quickstart done");
    Ok(())
}
