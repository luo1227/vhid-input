# vhid-input

在 Windows 上通过 `vhidinput` KMDF 驱动注入鼠标、键盘、滚轮与媒体键的 Rust
用户态库。报告经驱动送入 VHF/HID 栈，对系统而言是真实的 HID 输入设备。

## 前提：驱动

本库只是搬运工，设备本身来自同仓库的 `vhidinput.sys` 驱动，需先安装：

```powershell
bcdedit /set testsigning on          # 只需一次，然后重启
install\install.ps1                  # 管理员权限
```

驱动仅支持测试签名，因此只能跑在开启了测试签名的机器上。设备不在时
`VhidInput::open()` 返回 `Error::Open`。

## 引用

```toml
[dependencies]
vhid-input = { git = "https://github.com/luo1227/vhid-input", branch = "master" }
```

crate 位于仓库子目录 `user/`，仓库根有 workspace 清单，所以上面的写法即可
解析到它。或本地路径：

```toml
[dependencies]
vhid-input = { path = "../vhid-input/user" }
```

## 用法

实例式——自己管理句柄：

```rust
use vhid_input::{VhidInput, MouseButton};

let mut v = VhidInput::open()?;
v.move_to(100, 200)?;                 // 虚拟桌面像素坐标
v.click()?;                           // 左键单击
v.double_click(MouseButton::Right)?;
v.drag(100, 200, 400, 500)?;         // 左键拖拽
v.key_text("Hello, world!")?;         // US ASCII 逐字注入
v.chord(&[vhid_input::MOD_LCTRL], 0x04)?; // Ctrl+A
```

全局单例——进程内共享一个句柄，调用最短：

```rust
use vhid_input::{self as vhid, MouseButton};

vhid::click_at(100, 200, MouseButton::Left)?;
vhid::move_rel(-10, 5)?;
vhid::key_text("hi")?;
vhid::release_all()?;
```

## 线程安全

- `VhidInput` 不是 `Sync`，不要跨线程共享引用。要并发就每线程各 `open()`
  一份，或统一走全局单例自由函数（内部 `Mutex`，可任意线程调用）。
- 全局单例的句柄永不 `drop`，进程退出不会自动松键；需要时显式
  `release_all()`。

## 字符输入限制

`key_text` 按 US 键盘布局映射，只支持 ASCII 可打印字符与 `\n`/`\t`/回车/
退格。中文等多字节字符返回 `Error::UnsupportedChar`。

## 许可

MIT OR Apache-2.0。
