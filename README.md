# vhid-input

Windows 下的虚拟 HID 输入设备：一个 KMDF 驱动经 VHF（Virtual HID Framework）向系统注册一个多报告 HID 设备，用户态程序通过 `\\.\VhidInput` 写入报告，即可产生真实的鼠标、键盘、媒体键与滚轮输入。

用途是自动化与测试（模拟输入、驱动行为验证），不是采集或转发真实硬件输入。

## 架构

```
用户态 (Rust)                    内核态 (KMDF / C)
─────────────                    ────────────────
VhidInput::move_to() ─┐
VhidInput::press()   ─┼─ IOCTL_SUBMIT_REPORT ─▶ vhidinput.sys
VhidInput::media_..()─┘                          ├─ 队列 (256, 按报告 ID 合并滞后状态)
                                                 ├─ VhidInputPumpLocked()
                                                 └─ VhfReadReportSubmit()
                                                         │
                                                     hidclass / VHF
                                                         │
                                           HID 集合 COL01..COL04 ─▶ 系统消费
```

- 一个 `HID_XFER_PACKET` 对应一条完整输入报告（首字节是报告 ID）。
- `VhfReadReportSubmit` 只在 `VhidInputPumpLocked` 一处调用，处于 PASSIVE_LEVEL，由 `PumpLock` 串行化，不在任何自旋锁内调用。
- 每个提交使用轮转池中的独立槽位，不用队列项本身：VHF 在调用返回后仍可能读取缓冲区。

## HID 报告

设备标识 `VID 0x1209 / PID 0x0001`，四份报告放在同一个 HID 集合中，各自带报告 ID：

| ID | 集合 | 长度（不含 ID） | 内容 |
|----|------|------------------|------|
| 0x01 | Keyboard | 8 | 修饰键 + 保留字节 + 6 个键位（HID Usage Code） |
| 0x02 | Mouse（相对） | 6 | 5 键位 + int16 dx + int16 dy + int8 wheel |
| 0x03 | Mouse（绝对） | 6 | 5 键位 + uint16 x + uint16 y + int8 wheel，0..32767 映射整个虚拟桌面 |
| 0x04 | Consumer Control | 1 | 6 个 1 位标志 + 2 位填充 |

媒体键位序（报告 0x04 的 `data[0]`）：`bit0` Volume Up、`bit1` Volume Down、`bit2` Mute、`bit3` Scan Next、`bit4` Scan Prev、`bit5` Play/Pause。

绝对坐标由 `move_to(x, y)` 按虚拟桌面归一化，多显示器下虚拟桌面可能从负坐标开始，因此传的是虚拟桌面坐标而不是主显示器坐标。

## 目录

```
analyze_dump.bat      用 kd.exe 分析崩溃转储（指向暂存的 PDB）
run.bat               构建驱动 + 构建并运行 selftest
driver/               KMDF 驱动
  mk.bat              独立构建（不依赖 MSBuild），产物写入 bin\
  vhidinput.c         设备/队列/pump/IOCTL
  vhidinput_report.c  HID 报告描述符与接口 GUID
  vhidinput.h         报告 ID、长度、IOCTL 与状态结构（用户态 ABI 来源）
  vhidinput.inf       驱动安装信息（DriverVer 见文件头）
install/install.ps1   测试签名安装 / 卸载
user/                 Rust 用户态 crate（库 + 示例）
```

## 构建

驱动：

```bat
driver\mk.bat
```

需要 VS 2022 x64 工具链与 Windows Kit 10.0.26100（路径写死在 `mk.bat` 里，换机器要改）。产物为 `bin\vhidinput.sys` 及同名 PDB。

用户态：

```bat
cd user
cargo build --release --examples
```

## 安装

需要开启测试签名（`bcdedit /set testsigning on` 后重启），并以管理员身份运行：

```powershell
install\install.ps1            # 安装/更新到当前批次构建的 .sys
install\install.ps1 -Uninstall # 卸载设备与驱动包
install\install.ps1 -Uninstall -RemoveCert  # 连同自签名证书一起移除
```

安装脚本会用自签名证书签 `.cat`，装入 Trusted Root / Trusted Publisher；重复安装时原地更新已有设备节点，不会叠加出第二个实例。若 `.sys` 正在使用，`pnputil` 返回 3010（需要重启），脚本视为成功。

## 示例程序

在 `user/` 下 `cargo run --example <名字> -- <参数>`：

| 示例 | 作用 |
|------|------|
| `selftest` | 分阶段自测：`status` / `center` / `moves` / `rel` / `input` / `media`，省略参数则全跑 |
| `serve` | 从标准输入读指令、循环注入的常驻进程 |
| `rawmove` | 只发绝对移动，最小验证路径 |
| `rawmedia` | 发一条媒体键报告（`<bitmask> [hold_ms]`），用于验证 COL04 链路 |
| `hidread` | 以 HID 方式打开集合并打印收到的输入报告（`[秒数]`） |
| `hidcheck` | 枚举 HID 集合，打印用途页/用途与报告长度 |
| `rawinput` | 通过 RawInput 观察系统实际收到的输入 |
| `kbdprobe` | 键盘注入的逐字节探查 |
| `cursor` | 读取当前光标位置 |

注意：`hidread` 以独占方式打开 `col04` 可读，`col01/col02/col03` 被系统驱动占用，打开会返回“拒绝访问”，这是预期行为。

## 诊断

驱动在每次提交后把计数器发布到注册表 `HKLM\SYSTEM\CurrentControlSet\Services\vhidinput`：

`SubmitIo`、`SubmitQ`、`PumpCalls`、`PumpOk`、`PumpBusy`、`PumpBad`、`PumpAbs`、`PumpKey`，以及 `LastSubmitId/Len/Data/Status` 与 `VhfHandleLive`。

判断“报告卡在哪一层”：

1. 用户态 `status()` 的 `submitted[报告ID]` 不增长 → 没进驱动；
2. 该值增长但系统没有反应 → 进到了驱动并提交给 VHF，问题在 HID 消费侧；
3. `PumpBusy` 增长 → VHF 背压，报告被丢弃（`dropped` 同时增长）。

内核侧另可用 `DbgPrint` 与 ETW（`hidclass` provider `{084136C3-DB79-4ACA-AEBC-5316A2234EE7}`）交叉验证。

## 状态

- 报告描述符与 HID 集合枚举正常（COL01..COL04，媒体集合 `usage_page=0x000C usage=0x0001 in=2 buttons=6`）。
- 媒体集合的注入链路已逐帧验证：`virtual` 提交 → VHF → `ReadFile` 收到 `[04,01]`（按下）与 `[04,00]`（释放）。
- 已安装的 `.sys` 若与源码不一致，需重启后才会加载新映像；同一台机器上对比 `bin\vhidinput.sys` 与 `C:\Windows\System32\drivers\vhidinput.sys` 的时间戳即可确认。

## 版本记录

变更以 git 提交为准，`git log --oneline` 查看。
