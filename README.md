# rshot

一个 Windows 截图工具，Rust 写的。冻屏框选（Snipaste 那种）、鼠标悬停自动锁定窗口、托盘常驻、全局热键。**截图全程不落盘，直接进剪贴板。**

> 边学 Rust 边做的自用项目。

**[⬇ 下载最新版 rshot.exe](https://github.com/idkwhatimdoing62/rshot/releases/latest/download/rshot.exe)** —— 双击即用，无需装 Rust。

## 功能

- **冻屏框选**：按热键后画面定格，拖出矩形选区，回车截取。
- **窗口自动锁定**：鼠标悬到哪个窗口，就自动红框框住它，单击即截该窗口。
- **多入口触发**：全局热键 / 双击托盘图标。
- **不落盘**：截图直接进系统剪贴板，`Ctrl+V` 即用，磁盘不留文件。
- **配置可改**：热键写在配置文件里，重启生效。

## 用法

启动后程序挂在后台（任务栏托盘有个取景框图标）。

| 操作 | 效果 |
|---|---|
| `Alt+A`（或双击托盘图标） | 冻屏，进入框选 |
| 移动鼠标（不按键） | 自动红框锁住光标下的窗口 |
| 单击 | 截取当前锁定的窗口 |
| 按住拖拽 | 手动画选区 |
| `Enter` | 确认截取（没画框时 = 整屏） |
| `Esc` | 取消，不截 |
| `Alt+D` | 退出程序 |

截完 `Ctrl+V` 粘贴即可。

## 构建 / 运行

需要 Windows + Rust 工具链。

```bash
# 开发运行（保留控制台，方便看日志）
cargo run

# 出成品（无控制台黑框）
cargo build --release
# 产物在 target\release\rshot.exe，双击即后台运行
```

**开机自启**（可选）：`Win+R` → 输 `shell:startup` → 把 `rshot.exe` 的快捷方式丢进去。

## 配置

首次运行生成配置文件：

```
C:\Users\<你>\AppData\Roaming\RShot\config\default-config.yml
```

```yaml
hotkey: Alt+A   # 截图热键
quit: Alt+D     # 退出热键
```

改完重启生效。

## 技术栈

| 库 | 用途 |
|---|---|
| [xcap](https://crates.io/crates/xcap) | 抓屏 |
| [arboard](https://crates.io/crates/arboard) | 剪贴板 |
| [global-hotkey](https://crates.io/crates/global-hotkey) | 全局热键 |
| [winit](https://crates.io/crates/winit) | 窗口 / 事件循环 |
| [softbuffer](https://crates.io/crates/softbuffer) | CPU 像素缓冲（冻屏遮罩，不走 GPU 纹理，无 2048 上限） |
| [tray-icon](https://crates.io/crates/tray-icon) | 系统托盘 |
| [windows](https://crates.io/crates/windows) | Win32：光标 / DPI / 窗口枚举 / DWM |
| serde + confy | 配置读写 |

## 已知限制

- **仅 Windows**。
- **多屏跟随已实现但未实测**：代码按多屏写了（截鼠标所在屏、遮罩弹到该屏），只在单屏验证过。
- 回车后剪贴板编码那一两秒是同步的（已用"先隐藏窗口"掩盖，未上线程）。
