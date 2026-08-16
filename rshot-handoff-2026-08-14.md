# rshot 工作交接

生成日期：2026-08-14

## 当前状态

- 工作区：`C:\yangxiaochen\rshot`
- GitHub：<https://github.com/idkwhatimdoing62/rshot>
- 分支：`main`，工作树干净，已与 `origin/main` 同步。
- 当前 HEAD：`397752a docs: clarify release and OCR boundaries`
- 最新 Release：`v0.2.15`，代码标签位于 `7bc05f6`：<https://github.com/idkwhatimdoing62/rshot/releases/tag/v0.2.15>
- 高精度 OCR 实现提交：`d1671d6`。
- v0.2.15 发布前已通过 60 项测试、Windows x64 便携构建、依赖审计和真实模型推理烟测。
- Release 发布后又推送了两个纯文档提交 `1a9ef1e`、`397752a`；没有重新创建 Release，程序代码未变化。

## 权威资料

不要在交接中复制这些文档的完整内容，直接阅读：

- 使用、构建、OCR、内存与限制：`C:\yangxiaochen\rshot\README.md`
- 当前设计基线、契约、质量场景、决策记录和 C4 图：`C:\yangxiaochen\rshot\DESIGN.md`
- OCR 依赖与制品来源：`C:\yangxiaochen\rshot\THIRD_PARTY_NOTICES.md`
- Azure 架构风格笔记：`C:\yangxiaochen\笔记\Azure架构风格-按约束选择系统组织方式.md`

上述 Azure 笔记刚补充了适用范围和 rshot 反例。结论是：Azure 列出的六种风格主要面向云端或分布式系统；rshot 更准确的定位是“事件循环驱动的模块化单体桌面应用 + 集中式会话编排 + 手写状态机 + 一次性 OCR 隔离进程”。

## 已确认的产品决定

- 截图会话只由全局热键进入；双击托盘只显示作者和当前设置。
- `C` 复制截图；`Enter` 不再作为复制兼容键，只用于提交文字输入。
- 同一冻结画面支持重新选择。
- 支持最多 8 张独立置顶贴图；贴图左键拖动只移动贴图。
- 图片复制同时尝试 DIB 和唯一临时 PNG，临时文件按 12 小时策略安全清理。
- 捕获、渲染、剪贴板和 OCR 失败按会话边界恢复，不能让常驻进程因单次失败退出。
- 默认 OCR 为内嵌 PP-OCRv6 small-det + medium-rec；主进程按需启动同一 exe 的一次性 worker，失败或空结果时回退 Windows OCR。
- 用户接受当前 OCR 正确率。正式 Release 需要保持低待命内存；只在 OCR 时短时加载模型。
- 用户偏好本地只保留最新 Release 制品。

## 需要保留的事实边界

- 20 秒只约束高精度 OCR worker；随后 Windows OCR 回退当前没有独立超时，完整操作可能超过 20 秒。
- 约 11.2 MiB Working Set / 1.9 MiB Private Bytes 的低内存基线只适用于无活动会话且无贴图的纯待命状态；贴图会持有各自 RGBA 和 Surface。
- 当前 `App` 集中拥有会话及窗口资源。模块已经按编辑、几何、渲染、剪贴板、OCR、贴图和 Windows 边界拆分，但还没有纯 reducer 状态机或完整 Port/Adapter 边界。以 `DESIGN.md` 的现状描述为准。
- OCR worker 是同制品、同版本、同步监督的一次性子进程，不是微服务，也不是 Web-Queue-Worker。

## 未决事项

- rshot 项目自身尚未声明开源许可证。不要代替所有者选择 MIT、Apache-2.0 或其他许可证。README、DESIGN 和 THIRD_PARTY_NOTICES 已如实标注；后续确定许可证时，需要同步根级 `LICENSE`、Cargo 元数据、发布脚本和 Release 包。
- DESIGN 的 P1 仍要求在其他 CPU 和 4K、近全屏、超大图场景测量 OCR 延迟与峰值，并评估异步返回、取消及 Windows OCR 回退超时。
- v0.2.15 Release 附件仍是标签时的文档版本；后续只有在用户明确要求创建新 Release 时再构建和发布新版。
- 早前为构建关闭过一个测试运行实例，之后是否重新启动未知；涉及运行测试时先检查进程状态。

## 接手顺序

1. 先运行 `git status --short --branch`，确认没有用户新改动。
2. 阅读 README 和 DESIGN 中与新任务直接相关的章节，不要重新总结全部设计。
3. 修改代码时保持现有外部行为和失败恢复契约，并按风险补测试。
4. 用户要求“推送”时才提交并推送；用户要求“创建 Release”时再升版本、构建便携制品、验证、打标签和发布。
5. 发布前确认测试、最终 exe 烟测、导入表审计、附件和 SHA-256；不要上传旧 `target\release` 制品。

## Suggested skills

- `diagnosing-bugs`：用户报告交互、OCR、内存或 Release 行为异常时，先建立可重复证据并定位原因。
- `tdd`：新增功能或修复缺陷时先写回归测试，再实现和整理。
- `code-review`：准备推送或发布前，按仓库文档约束与工程质量双轴检查变更。
- `codebase-design`：继续拆分 `App`、收口平台边界或设计异步 OCR 接口时使用。
- `domain-modeling`：新增或修改设计决定、术语、状态和所有权边界时使用。
- `write-direct-chinese`：更新 README、DESIGN 或学习笔记时使用直接、简洁的中文。
- `research`：需要比较新的 OCR 模型、运行时或外部架构资料，并要求基于一手资料形成仓库文档时使用。

## 沟通偏好

- 使用中文，先给结论，再说明证据和必要取舍。
- 用户通常希望直接执行明确的修改；不要把“更新文档”“推送 GitHub”“创建 Release”混成同一权限。
- 对架构名称保持准确：局部使用事件循环或 worker，不等于系统采用 Azure Event-driven 或 Web-Queue-Worker 风格。
