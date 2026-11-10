//! 常驻注入服务：在 127.0.0.1 上接受极简文本指令，便于从外部（例如 agent
//! 环境）驱动虚拟键鼠而不必在本机窗口里手工操作。
//!
//! 协议：每行一条，回复一行。
//!   status                 -> "ok protocol=<v> pending=<n> dropped=<n>"
//!   pos                    -> 当前光标像素坐标 "ok x y"
//!   move X Y               -> 绝对移动（虚拟桌面坐标）
//!   rel DX DY              -> 相对移动
//!   click BUTTON           -> 单击 left/right/middle/side1/side2
//!   down BUTTON / up BUTTON
//!   key USAGE [mod...]     -> 按下 usage（可带 1..3 位十六进制修饰键 usage），松开
//!   wheel DELTA
//!   keydown USAGE / keyup USAGE
//!   release                -> 释放所有键与按钮
//! 任何错误回复 "err ..."。

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

use vhid_input::{init_dpi_awareness, MouseButton, VhidInput};

fn parse_button(s: &str) -> Option<MouseButton> {
    Some(match s {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        "side1" => MouseButton::Side1,
        "side2" => MouseButton::Side2,
        _ => return None,
    })
}

type Args<'a> = std::str::SplitWhitespace<'a>;

fn next_i32(it: &mut Args) -> Result<i32, String> {
    it.next()
        .ok_or_else(|| "missing argument".to_string())?
        .parse::<i32>()
        .map_err(|e| e.to_string())
}

fn next_usage(it: &mut Args) -> Result<u8, String> {
    let s = it.next().ok_or_else(|| "missing argument".to_string())?;
    u8::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|e| e.to_string())
}

fn handle(v: &mut VhidInput, line: &str) -> Result<String, String> {
    let mut it = line.split_whitespace();
    let cmd = it.next().unwrap_or("");
    match cmd {
        "status" => {
            let s = v.status().map_err(|e| e.to_string())?;
            let proto = s.protocol_version;
            let pending = s.pending_reports;
            let dropped = s.dropped_reports;
            Ok(format!(
                "ok protocol={proto} pending={pending} dropped={dropped}"
            ))
        }
        "pos" => {
            let (x, y) = cursor_pos();
            Ok(format!("ok {x} {y}"))
        }
        "move" => {
            let x = next_i32(&mut it)?;
            let y = next_i32(&mut it)?;
            v.move_to(x, y).map_err(|e| e.to_string())?;
            Ok("ok".into())
        }
        "rel" => {
            let dx = next_i32(&mut it)?;
            let dy = next_i32(&mut it)?;
            v.move_rel(dx, dy).map_err(|e| e.to_string())?;
            Ok("ok".into())
        }
        "wheel" => {
            v.wheel(next_i32(&mut it)?).map_err(|e| e.to_string())?;
            Ok("ok".into())
        }
        "click" | "down" | "up" => {
            let b = parse_button(it.next().unwrap_or(""))
                .ok_or_else(|| "bad button".to_string())?;
            match cmd {
                "click" => v.mouse_press(b),
                "down" => v.mouse_down(b),
                _ => v.mouse_up(b),
            }
            .map_err(|e| e.to_string())?;
            Ok("ok".into())
        }
        "key" => {
            let u = next_usage(&mut it)?;
            // 修饰键在前，主键随后，松开顺序相反。
            let mods: Vec<u8> = it
                .filter_map(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .collect();
            for m in &mods {
                v.key_down(*m).map_err(|e| e.to_string())?;
            }
            v.press(u).map_err(|e| e.to_string())?;
            for m in mods.iter().rev() {
                v.key_up(*m).map_err(|e| e.to_string())?;
            }
            Ok("ok".into())
        }
        "keydown" => {
            v.key_down(next_usage(&mut it)?).map_err(|e| e.to_string())?;
            Ok("ok".into())
        }
        "keyup" => {
            v.key_up(next_usage(&mut it)?).map_err(|e| e.to_string())?;
            Ok("ok".into())
        }
        "release" => {
            v.release_all().map_err(|e| e.to_string())?;
            Ok("ok".into())
        }
        _ => Err(format!("unknown command: {cmd}")),
    }
}

fn cursor_pos() -> (i32, i32) {
    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

fn main() -> std::io::Result<()> {
    init_dpi_awareness();
    let mut v = VhidInput::open().expect("cannot open the VhidInput device");
    println!("listening on 127.0.0.1:5005");

    let listener = TcpListener::bind("127.0.0.1:5005")?;
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        println!("client {peer}");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let reply = match handle(&mut v, line.trim()) {
                Ok(r) => r,
                Err(e) => format!("err {e}"),
            };
            if stream.write_all(reply.as_bytes()).is_err() || stream.write_all(b"\n").is_err() {
                break;
            }
            let _ = stream.flush();
        }
        // 断开时释放所有键，避免外部失控后键位卡住。
        let _ = v.release_all();
        println!("client gone");
    }
    let _ = thread::yield_now();
    Ok(())
}
