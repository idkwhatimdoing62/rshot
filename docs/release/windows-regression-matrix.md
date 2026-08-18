# Windows release regression matrix

复制本文件到 docs/release/results/v{version}.md。状态只能填写 PASS、FAIL、BLOCKED；每个失败或阻塞项必须链接 GitHub Issue。

## Environment

| Field | Value |
| --- | --- |
| Version / commit | |
| Tester / date | |
| Windows edition / build | |
| CPU / memory | |
| Displays | |
| Display scale | |
| Upgrade source version | |

## Automated and artifact checks

| ID | Scenario | Expected | Status | Evidence / issue |
| --- | --- | --- | --- | --- |
| A-01 | Four-command quality gate | Format, Clippy, tests and Release build pass | | |
| A-02 | run-release-smoke.ps1 | OCR worker and clipboard consumer pass | | |
| A-03 | Upgrade configuration | Existing YAML loads and new defaults apply | | |
| A-04 | Consecutive session test | Two independent screenshot sessions start and close without stale state | | |
| A-05 | Release session driver | Release executable reports first capture, consecutive capture, pin coexistence and OCR-with-pin scenarios | | |

## Capture, display and interaction

| ID | Scenario | Expected | Status | Evidence / issue |
| --- | --- | --- | --- | --- |
| C-01 | First hotkey capture after startup | Overlay appears on cursor display with aligned pixels | | |
| C-02 | Five consecutive captures | Every capture opens once; no stale overlay or lost hotkey | | |
| C-03 | 100%, 125%, 150%, 200% scale | Selection and output use the same physical pixels | | |
| C-04 | Two displays with equal scale | Cursor display is captured and overlay matches it | | |
| C-05 | Two displays with mixed scale and negative coordinates | No offset, crop or diagonal corruption | | |
| C-06 | Chinese and English IME text annotation | Preedit, commit, backspace and caret behave correctly | | |

## Pins

| ID | Scenario | Expected | Status | Evidence / issue |
| --- | --- | --- | --- | --- |
| P-01 | Pin then start another capture | Existing pin is absent from frozen pixels and restored immediately afterward | | |
| P-02 | Run OCR while pins exist | Pins remain visible during OCR processing | | |
| P-03 | Drag, double-click, right-click and Escape | Drag moves; other close gestures remove only the target pin | | |
| P-04 | Eight pins then create a ninth | Existing pins and active screenshot remain intact | | |

## OCR

| ID | Scenario | Expected | Status | Evidence / issue |
| --- | --- | --- | --- | --- |
| O-01 | Chinese, English and mixed screenshot | Text is copied in reading order | | |
| O-02 | Model worker unavailable | Windows OCR fallback is reported and remains usable | | |
| O-03 | OCR timeout or invalid worker result | Session recovers and diagnostics contain an RSH-OCR code | | |

## Clipboard and upgrade

| ID | Scenario | Expected | Status | Evidence / issue |
| --- | --- | --- | --- | --- |
| B-01 | Paste into Paint or image editor | CF_DIB image is consumable | | |
| B-02 | Paste into Explorer or upload control | CF_HDROP PNG is consumable | | |
| B-03 | Clipboard temporarily occupied | Bounded failure preserves the screenshot session | | |
| B-04 | Upgrade from previous version | Configuration remains valid; old executable can be restored | | |
| B-05 | Export diagnostics | Report contains version and stable codes, not OCR text, pixels, titles or document paths | | |

## Evidence collection

Before interactive verification, run:

```powershell
.\scripts\collect-windows-regression-evidence.ps1 -OutputPath .\windows-environment.json
```

Attach the generated JSON to the release result or its blocking GitHub Issue. It records only OS and display topology metadata; it does not capture pixels, window titles, paths, clipboard contents, or OCR text. A scenario may be marked PASS only when the recorded environment actually represents that scenario. For example, a single-display snapshot cannot close C-04 or C-05.

The release smoke suite also invokes the hidden `--rshot-session-self-test <path>` process role. Its JSON result exercises production session orchestration with headless window adapters plus the real packaged OCR worker. It detects stale first/consecutive session state, capture-time pin restoration, and accidental pin hiding during OCR. It does not prove visual alignment, native pointer delivery, IME composition, or external application behavior.

## Release decision

| Field | Value |
| --- | --- |
| Automated gate | |
| Interactive matrix | |
| Open blocking issues | |
| Decision | |
