# rshot

一个轻量的 Windows 截图工具，使用 Rust 编写。支持冻屏框选、窗口自动锁定、图片文字识别、画笔标注、置顶贴图、托盘常驻和全局热键。

**[下载最新版本 rshot.exe](https://github.com/idkwhatimdoing62/rshot/releases/latest/download/rshot.exe)**

当前稳定版为 **[v0.2.15](https://github.com/idkwhatimdoing62/rshot/releases/tag/v0.2.15)**。该版本已启用完全离线的 PP-OCRv6 高精度识别路径，并通过 60 项自动测试、便携 Release 构建、依赖审计和真实模型推理烟测。

## 功能

- **冻屏框选**：按热键冻结当前屏幕，拖出矩形选区。
- **窗口自动锁定**：移动鼠标时自动红框锁定光标下的窗口，单击进入编辑。
- **编辑工具栏**：选区确定后显示浮动工具栏 `PEN / LINE / RECT / TEXT / COLOR / UNDO / COPY / OCR / PIN / SELECT / X`，操作不必记快捷键。
- **标注工具**：`PEN` 自由画笔、`LINE` 直线、`RECT` 矩形、`TEXT` 文字批注（点击画面后直接打字，回车提交，`Esc` 取消，退格删字，支持中文）；点 `COLOR` 弹出色板（红/橙/黄/绿/蓝/紫/白/黑），点色块切换当前颜色，`Esc` 或点其它地方关闭。
- **本地文字识别**：点击 `OCR` 或按 `O`，识别当前原始选区中的文字，复制到剪贴板后关闭截图界面；默认使用嵌入 `rshot.exe` 的 PP-OCRv6 小型检测模型和中型识别模型，兼顾截图文字准确率与内存，运行时完全离线，画笔等标注不会进入识别输入。输入最长边不超过 4096 像素、总量不超过 800 万像素；识别结果按区域坐标恢复阅读顺序，归一项目符号和成对引号，并只在原图像素确实存在空白时恢复混排空格，不按字符类别或整句语义猜写。高精度后端不可用时回退到 `Windows.Media.Ocr`，复制完成后会明确提示本次使用了系统后端。
- **多张置顶贴图**：点击 `PIN` 把选区变成无边框、始终置顶的参考窗口，最多同时保留 8 张。
- **重新选择**：点击 `SELECT` 或按 `R`，在同一张冻结画面上重新框选，不需要重新截图。
- **剪贴板输出**：同时尝试提供位图和临时 PNG 文件，适配聊天软件、文档、终端和资源管理器；至少一种格式成功且剪贴板正常关闭后才结束截图，输出未完成时保留当前内容供重试。
- **热键截图**：截图会话只通过全局热键触发；双击托盘图标查看作者和当前热键设置。
- **失败可重试**：鼠标、显示器匹配或屏幕捕获失败时显示稳定错误码并回到待命，不残留遮罩窗口或旧截图。
- **热键配置**：首次运行生成 YAML 配置文件，修改后重启生效。

## 使用

启动后程序常驻系统托盘。

| 阶段 | 操作 | 效果 |
|---|---|---|
| 待命 | `Alt+A` | 冻结鼠标所在屏幕；失败时显示错误码并保持待命 |
| 任意阶段 | 双击托盘 | 显示作者信息和当前截图/退出热键 |
| 选择 | 移动鼠标 | 自动锁定光标下的窗口 |
| 选择 | 单击窗口 / 拖拽 | 确定窗口或手动选区，进入编辑 |
| 编辑 | 点 `PEN`/`LINE`/`RECT`/`TEXT`（或 `B`/`N`/`M`/`T`） | 切换画笔 / 直线 / 矩形 / 文字工具 |
| 编辑 | 点 `COLOR` | 弹出二级色板，点色块选色（红/橙/黄/绿/蓝/紫/白/黑） |
| 编辑 | 文字工具点画面 | 开始输入文字，回车提交 / `Esc` 取消 / 退格删字 |
| 编辑 | 左键拖动 | 用当前工具和颜色画标注 |
| 编辑 | `Ctrl+Z` | 撤销上一条标注 |
| 编辑 | 点击 `COPY` / `C` / 右键 | 输出完成后关闭；失败时保留当前截图并提示重试 |
| 编辑 | 点击 `OCR` / `O` | 识别选区原图文字，复制文字到剪贴板并关闭 |
| 编辑 | 点击 `SELECT` / `R` | 保留冻结画面，重新框选 |
| 选择 / 编辑 | 点击 `PIN` / `P` | 将当前选区直接变成置顶贴图 |
| 任一贴图 | 左键拖动 | 独立移动当前贴图 |
| 任一贴图 | 点击右上角 `X` / 右键 / `Esc` | 只关闭当前贴图 |
| 任意截图阶段 | `Esc` | 取消 |
| 待命 | `Alt+D` | 退出程序 |

贴图存在时仍可继续按截图热键。进入新截图会话后，已有贴图会暂时隐藏且不会进入截图；复制、取消或生成新贴图后自动恢复。活动截图会话仍然最多只有一个。

## 构建

需要 Windows x86_64 和 Rust 工具链。本机没有校验通过的缓存且未指定本地目录时，`build.rs` 会下载 PP-OCRv6 小型检测模型、中型识别模型、字符表及官方 ONNX Runtime 1.28.0 Windows x64 CPU ZIP，逐个校验文件大小与 SHA-256，再把两份 ONNX 模型、字符表和所需运行时 DLL 嵌入同一个 `rshot.exe`。构建完成后的程序不需要外置模型文件或网络。

离线构建时，把两份模型和字符表放在同一目录并设置 `RSHOT_OCR_MODEL_DIR`；也可以使用 `OAR_HOME` 或 `%USERPROFILE%\.oar` 中已有且校验通过的制品。`RSHOT_OCR_RUNTIME_DIR` 可以指定官方运行时目录，也可以指定自编译的静态 CRT 运行时目录；后者需要包含 `onnxruntime.dll`、`onnxruntime_providers_shared.dll` 和逐文件 SHA-256 清单 `rshot-ocr-runtime.sha256`。未指定现有目录时，构建脚本把官方运行时缓存到 `target` 下。

```bash
cargo run
cargo build --release
```

Release 产物位于 `target\release\rshot.exe`，双击后在后台运行。项目的 Windows MSVC 构建默认使用静态 CRT；正式便携 Release 还应通过 `RSHOT_OCR_RUNTIME_DIR` 嵌入使用 `/MT` 构建并通过依赖审计的 ONNX Runtime。

准备好使用 `--enable_msvc_static_runtime` 构建的 ONNX Runtime 1.28.0 CPU DLL 后，用发布脚本生成 SHA-256 清单，验证 x64 架构、ONNX Runtime 版本与 API 导出，按 Windows 系统 DLL 白名单审计主程序和两份运行时 DLL，并用真实模型推理烟测最终制品：

```powershell
.\scripts\build-portable-release.ps1 -OrtRuntimeDir C:\path\to\onnxruntime-release
```

脚本显式锁定 `x86_64-pc-windows-msvc`，最终便携产物位于 `target\x86_64-pc-windows-msvc\release\rshot.exe`；不会因用户级 Cargo target 配置误审其他目录中的旧文件。

高精度 OCR 使用的模型和推理组件说明见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 配置

首次运行生成：

```text
C:\Users\<用户>\AppData\Roaming\RShot\config\default-config.yml
```

```yaml
hotkey: Alt+A
quit: Alt+D
diagnostics: true
```

`diagnostics: false` 可关闭捕获失败日志。开启时只在同一目录的 `capture-errors.log` 中记录时间、事件名和 `RSH-CAP-xxx` 错误码，不记录坐标、窗口标题或截图内容；文件达到 64 KiB 后停止追加，删除该文件后会重新记录。

## 已知限制

- 仅支持 Windows x86_64。
- 最多同时保留 8 张置顶贴图；达到上限时保留当前编辑内容并提示先关闭一张旧贴图。
- 默认 PP-OCRv6 组合覆盖简体中文、繁体中文、英文、日文及 46 种拉丁语系语言，不覆盖韩文；识别仍可能受字号、压缩、背景、字体和版面影响，不能保证逐字准确。`Windows.Media.Ocr` 回退路径的语言还取决于 Windows 用户语言顺序和已安装语言包。
- OCR 期间主界面同步等待一次性 worker 完成，当前不能手动取消；一次识别总超时为 20 秒，超时后主进程会终止并等待回收 worker，再尝试 `Windows.Media.Ocr` 回退。worker 还加入带 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Windows Job Object，主进程异常退出时由系统结束子进程。复杂大图仍可能短时占用较多内存并延迟界面响应。
- 多显示器坐标逻辑已实现，但仍需要在更多缩放组合下验证。
- 剪贴板文件格式使用独立临时 PNG，不会被下一次复制覆盖；程序每 12 小时清理一次超过 12 小时且未被当前剪贴板引用的文件。

## 内存

- 待命时不保留截图像素，也不加载 OCR 模型或推理引擎，适合长期驻留；当前机器启动后静置 3 秒约为 11.2 MiB Working Set、1.9 MiB Private Bytes，实际数值会随系统和托盘状态变化。
- 截图显示直接从原始 RGBA 图像渲染，不再额外保存一份整屏显示副本。
- 复制和置顶时会优先复用原图，减少高分辨率截图的瞬时复制。
- 待命时只有一个常驻进程，主进程不初始化 PP-OCR 模型或推理引擎，也不直接链接 ONNX Runtime 或 DirectML。执行 OCR 时，同一个 `rshot.exe` 以 `--rshot-ocr-worker` 启动一次性子进程；仅 worker 按需校验并原子提取嵌入的 CPU 运行时 DLL 到以两份 DLL 组合 SHA-256 前缀命名的 `%LOCALAPPDATA%\RShot\ocr-runtime-win-x64-1.28.0-*` 目录；缺少 `LOCALAPPDATA` 时使用系统临时目录。worker 在识别完成、失败或达到 20 秒超时后退出；超时时主进程终止并等待回收，主进程异常退出时由带 kill-on-close 的 Windows Job Object 回收。高精度后端失败时，本次识别回退到系统 OCR，并区分“模型无文字”和“模型不可用”提示实际后端。
- 默认模型请求最长边不超过 4096 像素、总量不超过 800 万像素；检测阶段把最长边限制为 960 像素，原始轮廓候选最多 1000 个。这样优先控制 8 GB 设备峰值，同时避免在置信度过滤前用过低候选数静默截断正文；异常密集图片由 20 秒 worker 熔断降级。高分屏整页的极小文字应缩小选区后再识别。RGBA 输入和推理缓冲只在本次识别期间存在。推理 session 固定 intra-op=2、inter-op=1、顺序执行并关闭 memory pattern，区域批次固定为 2，在逐区域串行和四区域高峰值之间折中。Windows 回退路径仍只对小图执行 200 万像素预算内的最多 2 倍增强。
- 当前机器用 829×313 和 894×235 两张混排样例验证最终便携 Release worker，逐字结果均与原图一致；第二张连续 3 轮单次约 1.33～1.47 秒，峰值约 371.6～372.7 MiB Working Set、309.9～310.4 MiB Private Bytes，随后进程退出并释放。42 行 1920×1080 合成混排页约 9.14 秒、382.0 MiB Private Bytes；800 万像素空白图约 0.40 秒、280.2 MiB Private Bytes。这组数据只对应当前机器、构建和样例，不代表其他设备、图片或版本的上限。
- 每张贴图只保留自己的最终 RGBA 图和绘图表面，关闭后立即释放；8 张上限避免误操作造成无界内存增长。
