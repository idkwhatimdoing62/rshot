# Third-party notices

rshot 的高精度本地 OCR 功能包含或使用以下开源组件与模型。运行时不向这些项目或其维护者发送图片、识别文字或遥测数据。

| 项目 | 用途 | 许可证 | 来源 |
| --- | --- | --- | --- |
| OAR OCR 0.9.1 | Rust OCR 推理与图像处理 | Apache License 2.0 | <https://github.com/GreatV/oar-ocr> |
| ort 2.0.0-rc.13 | Rust ONNX Runtime 绑定 | MIT OR Apache-2.0 | <https://github.com/pykeio/ort> |
| PaddleOCR / PP-OCRv6 small-det + medium-rec | 文字检测、识别模型与字符字典 | Apache License 2.0 | <https://github.com/PaddlePaddle/PaddleOCR> |
| ONNX Runtime 1.28.0 CPU | 本地 ONNX 模型推理；便携 Release 从固定上游版本以 `/MT` 构建，DLL 由 OCR worker 按需提取并动态加载 | MIT License | <https://github.com/microsoft/onnxruntime> |

正式便携 Release 使用 `/MT` 自编译 ONNX Runtime，并按 Windows 系统 DLL 白名单审计主程序和运行时 DLL 的导入表，避免依赖外置 VC++、MinGW、GPU 或其他第三方运行时；发布门禁还验证 x64 架构、ONNX Runtime 版本与 API 导出，并执行真实模型推理烟测。源码的默认开发构建仍可使用官方动态 CRT 运行时；其依赖缺失或 ONNX Runtime 动态加载失败时，不会阻止主进程启动，本次识别会明确提示并改用 Windows 系统 OCR。发布制品不包含 DirectML。

各项目的商标和模型归其权利人所有。发布制品应同时保留本文件、rshot 项目许可证，以及 OAR OCR、ort、PaddleOCR/模型和 ONNX Runtime 对应的许可证与版权声明；上游链接用于核对来源和版本。
