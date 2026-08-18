# Changelog

版本遵循 Semantic Versioning。候选版本使用 -rc.N 后缀；正式版本只包含已经通过完整 Windows 回归矩阵的候选版本。

## 0.3.0-rc.1 - 2026-08-18

### Changed

- 深化截图操作、交互、输出、贴图集合、捕获尝试、OCR、剪贴板发布和临时产物生命周期边界。
- 贴图在截图像素捕获完成后立即恢复，不再覆盖后续同步 OCR 时间。
- 贴图改为双击或右键关闭，不显示窗口关闭按钮。
- 剪贴板发布增加双格式部分成功、有限重试和事务确定性。

### Added

- GitHub Actions Windows 强制质量门禁。
- 连续捕获、OCR 制品和剪贴板消费 Release 烟测。
- 统一稳定错误码和隐私安全诊断导出。
- 版本化 Windows 回归矩阵及 Release 结果记录。

### Known limitations

- 多显示器、混合 DPI、IME 和贴图交互仍需在真实交互式 Windows 会话执行版本化回归矩阵。
- 当前候选版本没有自动更新器和代码签名。
