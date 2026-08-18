# rshot

轻量、离线的 Windows 截图工具，支持区域截图、窗口锁定、标注、OCR 和置顶贴图。

## 下载

从 [GitHub Releases](https://github.com/idkwhatimdoing62/rshot/releases) 下载最新版本的 `rshot.exe`。当前稳定版是 [v0.2.15](https://github.com/idkwhatimdoing62/rshot/releases/tag/v0.2.15)，候选版本会标记为 Pre-release。

下载后直接运行，程序会常驻系统托盘。

## 核心功能

- 框选区域或自动锁定鼠标下的窗口
- 画笔、直线、矩形、文字、颜色和撤销
- 图片复制到剪贴板
- 完全离线的中英文 OCR，失败时回退到 Windows OCR
- 最多保留 8 张独立置顶贴图
- 在同一张冻结画面上重新选择区域

## 快捷操作

| 操作 | 快捷键或手势 |
| --- | --- |
| 开始截图 | `Alt+A` |
| 退出程序 | `Alt+D` |
| 取消截图 | `Esc` |
| 复制图片 | `C`、右键或 `COPY` |
| OCR | `O` 或 `OCR` |
| 生成贴图 | `P` 或 `PIN` |
| 重新选择 | `R` 或 `SELECT` |
| 撤销标注 | `Ctrl+Z` |
| 关闭单张贴图 | 双击、右键或 `Esc` |

截图和退出热键可在配置文件中修改。

## 配置

首次运行会生成：

```text
C:\Users\<用户名>\AppData\Roaming\RShot\config\default-config.yml
```

默认配置：

```yaml
hotkey: Alt+A
quit: Alt+D
diagnostics: true
```

修改后重启 rshot 生效。将 `diagnostics` 设为 `false` 可关闭故障日志。

## 诊断与反馈

遇到问题时可以导出隐私安全的诊断报告：

```powershell
.\rshot.exe --export-diagnostics .\rshot-diagnostics.txt
```

报告只包含版本、系统类型和稳定错误码，不包含截图、OCR 文字、窗口标题或文档路径。提交 [GitHub Issue](https://github.com/idkwhatimdoing62/rshot/issues) 时，请附上报告、复现步骤和 Windows 显示缩放信息。

## 开发

需要 Windows x86_64 和 Rust 工具链：

```powershell
cargo run
cargo build --release
```

更详细的信息：

- [设计与行为约束](DESIGN.md)
- [发布、升级和回退](RELEASE.md)
- [版本记录](CHANGELOG.md)
- [第三方组件与许可证](THIRD_PARTY_NOTICES.md)
