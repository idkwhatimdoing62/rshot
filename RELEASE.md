# Release policy

## Versioning

- 使用 Semantic Versioning。
- 候选版本：vMAJOR.MINOR.PATCH-rc.N。
- 正式版本：vMAJOR.MINOR.PATCH，必须与通过完整回归矩阵的候选版本来自同一提交。
- 每次用户可见行为变化更新 CHANGELOG.md。

## Assets

- Git tag 与 Cargo 版本一致并带 v 前缀。
- Windows x64 便携文件名：rshot-v{version}-windows-x86_64.exe。
- 同时发布 SHA256SUMS.txt。
- GitHub 候选版本必须标记为 pre-release；正式版本不得标记为 pre-release。

## Quality gate

干净检出必须通过：

~~~powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
.\scripts\run-release-smoke.ps1
~~~

GitHub Actions 强制执行前四项及适合无交互 runner 的 OCR、剪贴板烟测。本机交互式 Windows 执行完整烟测和版本化回归矩阵。

## Configuration and data upgrades

- 配置由 serde(default) 读取；新增字段必须提供默认值，旧配置无需人工迁移。
- 删除或改变字段语义属于不兼容变化，必须提供显式迁移并提升主版本。
- OCR 运行时目录包含运行时版本和内容摘要；新版不能覆盖旧版目录。
- 剪贴板临时 PNG 只按严格白名单清理；旧版 %TEMP%\rshot.png 仅按精确路径处理。
- Release 不删除用户配置、诊断文件或未知临时文件。

## Regression record

复制 docs/release/windows-regression-matrix.md 为 docs/release/results/v{version}.md，填写机器、系统、显示器、DPI、结果和 issue。记录必须与 Release 提交一起保存。

正式版本要求所有必测项目为 PASS。候选版本允许明确记录 BLOCKED 或 FAIL，但 Release notes 必须列出对应风险和 issue。

## Rollback

1. 从 GitHub Releases 下载上一个通过矩阵的正式版本及 SHA256SUMS.txt。
2. 校验 SHA-256 后关闭 rshot，替换单个 rshot.exe。
3. 保留配置和诊断文件；旧版不能读取的新字段会被忽略。
4. 回退原因记录为 GitHub Issue，并在下一版本 Changelog 中引用。
5. 不移动 tag、不覆盖既有 Release 资产；修复使用新版本号。
