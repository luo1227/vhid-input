//! 驱动压力测试。需先安装驱动并启动服务。
//!
//! 分三个阶段，逐层加压：
//!
//! * `single`  单线程：一个 `VhidInput` 连续注入 N 条绝对移动报告，
//!             观察队列积压与丢弃。
//! * `concurrent`  多线程：T 个线程各开一个 `VhidInput`，同时提交，
//!             验证驱动侧 PumpLock 与队列在并发下是否稳定、是否会丢。
//! * `abnormal`  异常路径：提交后立刻丢弃句柄（模拟崩溃）、提交非法
//!             报告 id、设备重新枚举。
//!
//! 用法：`stress.exe <阶段> <次数> <线程数>`，阶段为 single、concurrent、
//! abnormal 或 all。默认 `all 20000 4`。
//!
//! 每一阶段前后都读 `status()`。判断口径：
//! - `dropped` 增长是正常的（VHF 背压），但要能说清属于哪类；
//! - `pending` 稳定后应回落到 0，长期不落说明 pump 卡死；
//! - 任一阶段进程崩溃 / 蓝屏，即视为该阶段失败。

use std::sync::atomic::{AtomicU64, AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use vhid_input::{init_dpi_awareness, virtual_rect, VhidInput};

/// Status 是 #[repr(C, packed)]，字段不能直接取引用；统一拷贝出来。
#[derive(Clone, Copy, Default)]
struct Snap {
    protocol: u32,
    pending: u32,
    dropped: u32,
    max_pending: u32,
}

fn snap(s: &vhid_input::Status) -> Snap {
    // Status 实现了 Copy，整体按值拷贝（packed 结构体可整体拷贝），
    // 再读局部变量就不会触发 unaligned 借用错误。
    let s = *s;
    Snap {
        protocol: s.protocol_version,
        pending: s.pending_reports,
        dropped: s.dropped_reports,
        max_pending: s.max_pending,
    }
}

fn read_status(label: &str) -> Snap {
    let s = match VhidInput::open() {
        Ok(v) => match v.status() {
            Ok(s) => s,
            Err(e) => {
                println!("[{label}] status 读取失败: {e}");
                return Snap::default();
            }
        },
        Err(e) => {
            println!("[{label}] open 失败: {e}");
            return Snap::default();
        }
    };
    let sn = snap(&s);
    println!(
        "[{label}] protocol={} pending={} dropped={} max={}",
        sn.protocol, sn.pending, sn.dropped, sn.max_pending
    );
    sn
}

/// 单线程连续提交 `count` 条绝对移动报告。
fn stage_single(count: u64) {
    println!("== stage single: {count} 条绝对移动 ==");
    let before = read_status("single.before");
    let (w, h) = virtual_rect();
    let mut v = match VhidInput::open() {
        Ok(v) => v,
        Err(e) => {
            println!("[single] open 失败: {e}");
            return;
        }
    };
    let t0 = Instant::now();
    let mut errors = 0u64;
    for i in 0..count {
        // 在虚拟桌面内画一条对角线，坐标合法且不会把光标钉在角落。
        let x = ((i % 1000) as i32 * w / 1000).clamp(0, w - 1);
        let y = ((i % 1000) as i32 * h / 1000).clamp(0, h - 1);
        if let Err(e) = v.move_to(x, y) {
            errors += 1;
            if errors <= 3 {
                println!("[single] 提交失败 #{i}: {e}");
            }
        }
    }
    let dt = t0.elapsed();
    println!(
        "[single] 完成 {count} 条，用时 {:.1}ms（{:.0} 条/s），IOCTL 错误 {errors}",
        dt.as_secs_f64() * 1e3,
        count as f64 / dt.as_secs_f64()
    );
    // 让 pump 把剩余积压排空：驱动只在 IOCTL 路径上 pump，需再发几次。
    drain(&mut v, &before);
}

/// 并发提交后强制排空队列，并报告净变化。
fn drain(v: &mut VhidInput, before: &Snap) {
    let mut last_pending = u32::MAX;
    for _ in 0..200 {
        let _ = v.move_to(0, 0); // 每一次 IOCTL 都会 pump 最多 16 条
        if let Ok(s) = v.status() {
            let sn = snap(&s);
            if sn.pending == 0 || sn.pending == last_pending {
                last_pending = sn.pending;
                if sn.pending == 0 {
                    break;
                }
            } else {
                last_pending = sn.pending;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    let after = match v.status() {
        Ok(s) => snap(&s),
        Err(_) => return,
    };
    println!(
        "[single.after] pending={} dropped={}（相对 before 增加 dropped={}）",
        after.pending,
        after.dropped,
        after.dropped.saturating_sub(before.dropped)
    );
    if after.pending != 0 {
        println!("[single] 警告：排空后仍有 {} 条未消费，pump 可能卡住", after.pending);
    }
}

/// 多线程并发：T 个线程各开一个句柄，同时提交 `per_thread` 条。
fn stage_concurrent(threads: usize, per_thread: u64) {
    println!("== stage concurrent: {threads} 线程 × {per_thread} 条 ==");
    let before = read_status("concurrent.before");
    let (w, h) = virtual_rect();
    let start = Arc::new(AtomicBool::new(false));
    let total_errors = Arc::new(AtomicU64::new(0));
    let total_ok = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();
    for t in 0..threads {
        let start = Arc::clone(&start);
        let total_errors = Arc::clone(&total_errors);
        let total_ok = Arc::clone(&total_ok);
        handles.push(thread::spawn(move || {
            let mut v = match VhidInput::open() {
                Ok(v) => v,
                Err(e) => {
                    println!("[thread {t}] open 失败: {e}");
                    total_errors.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            };
            // 等所有线程就绪后同时开跑，制造真实并发。
            while !start.load(Ordering::Acquire) {
                thread::yield_now();
            }
            for i in 0..per_thread {
                let x = ((i as i32 + t as i32 * 97) % 1000 * w / 1000).clamp(0, w - 1);
                let y = ((i as i32 + t as i32 * 53) % 1000 * h / 1000).clamp(0, h - 1);
                match v.move_to(x, y) {
                    Ok(()) => {
                        total_ok.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        total_errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    let t0 = Instant::now();
    start.store(true, Ordering::Release);
    for h in handles {
        let _ = h.join();
    }
    let dt = t0.elapsed();
    println!(
        "[concurrent] 完成 {} 条，用时 {:.1}ms，IOCTL 错误 {}",
        total_ok.load(Ordering::Relaxed),
        dt.as_secs_f64() * 1e3,
        total_errors.load(Ordering::Relaxed)
    );
    // 排空：任取一个句柄反复 pump。
    if let Ok(mut v) = VhidInput::open() {
        drain_quiet(&mut v, &before);
    }
}

fn drain_quiet(v: &mut VhidInput, before: &Snap) {
    for _ in 0..400 {
        let _ = v.move_to(0, 0);
        if let Ok(s) = v.status() {
            if snap(&s).pending == 0 {
                break;
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    if let Ok(after) = v.status() {
        let sn = snap(&after);
        println!(
            "[concurrent.after] pending={} dropped={}（增加 dropped={}）",
            sn.pending,
            sn.dropped,
            sn.dropped.saturating_sub(before.dropped)
        );
        if sn.pending != 0 {
            println!("[concurrent] 警告：排空后仍有 {} 条未消费", sn.pending);
        }
    }
}

/// 异常路径：边提交边丢弃句柄（模拟崩溃），以及非法报告 id。
fn stage_abnormal() {
    println!("== stage abnormal ==");

    // 1. 提交后立刻 drop，期间不断开关句柄。Drop 会尽力 release_all。
    for i in 0..200 {
        match VhidInput::open() {
            Ok(mut v) => {
                let _ = v.move_to(100 + i, 100);
                let _ = v.mouse_down(vhid_input::MouseButton::Left);
                // 不 mouse_up，drop 时 release_all 应抬起左键。
            }
            Err(e) => println!("[abnormal.1] open 失败 #{i}: {e}"),
        }
    }
    println!("[abnormal.1] 200 次 打开→提交→不带松键 drop 完成");

    // 2. 非法报告 id：直接走库的原始提交路径不可用（lib 不暴露），
    //    改为验证库层拒绝：key_text 非 ASCII、mouse 正常路径不 panic。
    let mut v = match VhidInput::open() {
        Ok(v) => v,
        Err(e) => {
            println!("[abnormal.2] open 失败: {e}");
            return;
        }
    };
    match v.key_text("中") {
        Ok(()) => println!("[abnormal.2] 非 ASCII 竟然成功（不应发生）"),
        Err(e) => println!("[abnormal.2] 非 ASCII 被拒绝，符合预期: {e}"),
    }
    // 修饰键满、键位满：连按 8 个非修饰键，第 7 个应报错。
    for k in 0..8u8 {
        if v.key_down(0x04 + k).is_err() {
            println!("[abnormal.2] 第 {} 个键报错（键位满），符合预期", k + 1);
            break;
        }
    }
    let _ = v.release_all();

    // 3. 设备可用性：异常路径后仍能读到状态。
    match v.status() {
        Ok(s) => {
            let sn = snap(&s);
            println!(
                "[abnormal.3] 状态可读，pending={} dropped={} protocol={}",
                sn.pending, sn.dropped, sn.protocol
            );
        }
        Err(e) => println!("[abnormal.3] 状态读取失败（设备可能已挂）: {e}"),
    }
}

fn main() {
    init_dpi_awareness();
    let mut args = std::env::args().skip(1);
    let stage = args.next().unwrap_or_else(|| "all".into());
    let count: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let threads: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(4);

    let (w, h) = virtual_rect();
    println!("virtual desktop: {w}x{h}");

    // 起始基线。
    read_status("baseline");

    if stage == "all" || stage == "single" {
        stage_single(count);
    }
    if stage == "all" || stage == "concurrent" {
        stage_concurrent(threads, count / threads.max(1) as u64);
    }
    if stage == "all" || stage == "abnormal" {
        stage_abnormal();
    }

    read_status("final");
    println!("stress done");
}
