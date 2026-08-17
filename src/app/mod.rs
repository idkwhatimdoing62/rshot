mod capture_operation;
mod clipboard;
mod diagnostics;
mod editor;
mod geometry;
mod handler;
mod ocr;
mod ocr_worker;
mod pinned;
mod render;
mod state;
mod windows_adapter;

use capture_operation::*;
use clipboard::*;
use diagnostics::*;
use editor::*;
use geometry::*;
use ocr::*;
use ocr_worker::*;
use pinned::*;
#[cfg(test)]
use render::*;
use state::*;
use windows_adapter::*;

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use softbuffer::{Context, Surface};
use std::collections::HashMap;
use std::error::Error;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};
use xcap::Monitor;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Config {
    hotkey: String,
    quit: String,
    diagnostics: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: "Alt+A".into(),
            quit: "Alt+D".into(),
            diagnostics: true,
        }
    }
}

const AUTHOR: &str = "idkwhatimdoing62";
const REPOSITORY_URL: &str = "https://github.com/idkwhatimdoing62/rshot";

fn build_about_message(hotkey: &str, quit: &str, diagnostics: bool) -> String {
    format!(
        "rshot v{}\n\n作者：{}\n项目：{}\n\n当前设置\n截图热键：{}\n退出热键：{}\n诊断日志：{}\n\n截图只能通过全局热键触发。\n修改配置后请重启程序。",
        env!("CARGO_PKG_VERSION"),
        AUTHOR,
        REPOSITORY_URL,
        hotkey,
        quit,
        if diagnostics { "开启" } else { "关闭" },
    )
}

#[derive(Default)]
struct App {
    // 两个热键的 id，用来分辨收到的是哪一个
    shot_id: u32,
    quit_id: u32,
    about_message: String,
    diagnostics_enabled: bool,
    capture_operation: Option<CaptureOperation>,

    pins: HashMap<WindowId, PinnedWindow>,
    last_temp_cleanup: Option<Instant>,
}

impl App {
    #[cfg(test)]
    fn selection(&self) -> Option<((i32, i32), (i32, i32))> {
        self.capture_operation
            .as_ref()
            .and_then(CaptureOperation::selection)
    }

    fn begin_capture_attempt(&mut self) {
        self.close_overlay();
        self.capture_operation = Some(CaptureOperation::begin());
    }

    /// 截图热键触发：截鼠标那块屏 + 弹全屏遮罩，进入框选
    fn open_overlay(&mut self, event_loop: &dyn ActiveEventLoop) {
        // 每个热键事件都是全新的尝试；先清掉任何不完整的旧会话。
        self.begin_capture_attempt();

        // 1. 鼠标坐标（进程已 DPI aware，拿的是物理像素）
        let Some(cursor) = cursor_position() else {
            self.handle_capture_failure(CaptureFailureStage::ReadCursor);
            return;
        };

        // 2. 分别定位截图后端和遮罩窗口使用的显示器。
        let Ok(monitor) = Monitor::from_point(cursor.0, cursor.1) else {
            self.handle_capture_failure(CaptureFailureStage::LocateCaptureMonitor);
            return;
        };
        let (cx, cy) = cursor;
        let target = event_loop.available_monitors().find_map(|monitor| {
            let (Some(position), Some(mode)) = (monitor.position(), monitor.current_video_mode())
            else {
                return None;
            };
            let size = mode.size();
            if cx >= position.x
                && cy >= position.y
                && cx < position.x + size.width as i32
                && cy < position.y + size.height as i32
            {
                Some((monitor, (position.x, position.y)))
            } else {
                None
            }
        });
        let Some((target, origin)) = target else {
            self.handle_capture_failure(CaptureFailureStage::MatchOverlayMonitor);
            return;
        };

        // 3. 截鼠标所在屏：隐藏旧贴图后获取并保留一份原始 RGBA。
        // 旧贴图在整个新会话期间保持隐藏，既不会进入截图，也不会盖住选择遮罩。
        self.set_pins_visible(false);
        let img = match monitor.capture_image() {
            Ok(img) => img,
            Err(_) => {
                self.handle_capture_failure(CaptureFailureStage::CaptureImage);
                return;
            }
        };

        // 4. 建全屏无边框窗口钉到已经匹配的显示器。
        // 弹遮罩之前把所有可见窗口的矩形拍个快照（之后遮罩会盖住一切，就点不到底下窗口了）
        let windows = visible_window_rects();

        let window: Rc<dyn Window> = match event_loop.create_window(
            WindowAttributes::default()
                .with_fullscreen(Some(winit::monitor::Fullscreen::Borderless(Some(target)))),
        ) {
            Ok(window) => Rc::from(window),
            Err(error) => {
                self.handle_session_failure(SessionFailure::new(
                    SessionFailureStage::CreateWindow,
                    error,
                ));
                return;
            }
        };
        let context = match Context::new(window.clone()) {
            Ok(context) => context,
            Err(error) => {
                drop(window);
                self.handle_session_failure(SessionFailure::new(
                    SessionFailureStage::CreateContext,
                    error,
                ));
                return;
            }
        };
        let surface = match Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(error) => {
                drop(context);
                drop(window);
                self.handle_session_failure(SessionFailure::new(
                    SessionFailureStage::CreateSurface,
                    error,
                ));
                return;
            }
        };
        #[allow(deprecated)]
        window.set_ime_allowed(true); // 让遮罩窗口能接收输入法组合（拼音候选窗）
        window.request_redraw(); // 主动要首帧，否则黑底白窗
        let Some(operation) = self.capture_operation.take() else {
            self.handle_capture_failure(CaptureFailureStage::CaptureImage);
            return;
        };
        let captured = CapturedSession::new(
            img,
            Box::new(LiveCaptureWindow::new(window, surface)),
            cursor,
            origin,
            windows,
        );
        self.capture_operation = Some(operation.attach_capture(captured));
    }

    /// 确认截图：至少一种图片格式写入成功才结束会话；全部失败则恢复编辑界面。
    fn confirm(&mut self) {
        let Some(operation) = &mut self.capture_operation else {
            return;
        };
        operation.set_window_visible(false);
        let outcome = match operation.copy_source() {
            Ok(source) => image_to_clipboard(source.image.as_ref(), source.owner),
            Err(_) => {
                self.restore_after_image_copy_failure("无法生成要复制的截图图像。");
                return;
            }
        };
        if outcome.succeeded() {
            println!("图片已写入剪贴板：{}", outcome.published().description());
            self.close_overlay();
        } else {
            self.restore_after_image_copy_failure(&outcome.failure_message());
        }
    }

    fn restore_after_image_copy_failure(&self, detail: &str) {
        show_message(
            &format!(
                "复制图片失败，当前截图和标注已保留。\n\n{detail}\n\n请稍后重试；若剪贴板被其他程序占用，请先关闭相关程序。"
            ),
            true,
        );
        if let Some(operation) = &self.capture_operation {
            operation.set_window_visible(true);
            operation.request_redraw();
        }
    }

    /// 识别当前原始选区中的文字并写入文字剪贴板。标注层不会参与 OCR。
    fn copy_ocr_text(&mut self) {
        let Some(operation) = &mut self.capture_operation else {
            return;
        };
        operation.set_window_visible(false);
        let (result, clipboard_owner) = match operation.ocr_source() {
            Ok(source) => (
                recognize_image_text(source.frozen_image, source.selection),
                Some(source.owner),
            ),
            Err(_) => {
                operation.set_window_visible(true);
                operation.request_redraw();
                return;
            }
        };
        match result {
            Ok(recognition) if recognition.text.is_empty() => {
                let backend_note = match recognition.fallback_reason {
                    Some(OcrFallbackReason::ModelReturnedNoText) => {
                        "\n高精度模型未识别到文字，已改用 Windows 系统 OCR。"
                    }
                    Some(OcrFallbackReason::ModelUnavailable) => {
                        "\n高精度模型本次不可用，已改用 Windows 系统 OCR。"
                    }
                    None => "",
                };
                show_message(
                    &format!("未识别到文字。\n请缩小选区，并确保文字足够清晰。{backend_note}"),
                    false,
                );
                if let Some(operation) = &self.capture_operation {
                    operation.set_window_visible(true);
                    operation.request_redraw();
                }
            }
            Ok(recognition)
                if clipboard_owner
                    .is_some_and(|owner| text_to_clipboard(&recognition.text, owner)) =>
            {
                let fallback_reason = recognition.fallback_reason;
                self.close_overlay();
                match fallback_reason {
                    Some(OcrFallbackReason::ModelReturnedNoText) => show_message(
                        "高精度模型未识别到文字，已改用 Windows 系统 OCR 并复制文字。",
                        false,
                    ),
                    Some(OcrFallbackReason::ModelUnavailable) => show_message(
                        "高精度模型本次不可用，已改用 Windows 系统 OCR 并复制文字。",
                        false,
                    ),
                    None => {}
                }
            }
            Ok(recognition) => {
                let backend_note = match recognition.fallback_reason {
                    Some(OcrFallbackReason::ModelReturnedNoText) => {
                        "\n高精度模型未识别到文字，本次结果来自 Windows 系统 OCR。"
                    }
                    Some(OcrFallbackReason::ModelUnavailable) => {
                        "\n高精度模型本次不可用，本次结果来自 Windows 系统 OCR。"
                    }
                    None => "",
                };
                show_message(
                    &format!("文字已识别，但写入剪贴板失败，请重试。{backend_note}"),
                    true,
                );
                if let Some(operation) = &self.capture_operation {
                    operation.set_window_visible(true);
                    operation.request_redraw();
                }
            }
            Err(error) => {
                show_message(&format!("文字识别失败：\n{error}"), true);
                if let Some(operation) = &self.capture_operation {
                    operation.set_window_visible(true);
                    operation.request_redraw();
                }
            }
        }
    }
    fn pin(&mut self, event_loop: &dyn ActiveEventLoop) {
        if !has_pin_capacity(self.pins.len()) {
            if let Some(operation) = &self.capture_operation {
                operation.set_window_visible(false);
            }
            show_message(
                &format!(
                    "最多同时保留 {MAX_PINNED_WINDOWS} 张置顶贴图。\n请取消当前截图，关闭一张旧贴图后再试。"
                ),
                false,
            );
            if let Some(operation) = &self.capture_operation {
                operation.set_window_visible(true);
                operation.request_redraw();
            }
            return;
        }
        let Some(operation) = self.capture_operation.as_mut() else {
            return;
        };
        let Ok(plan) = operation.prepare_pin() else {
            return;
        };
        let prepared = match PreparedPinnedWindow::create(event_loop, plan.position, plan.size) {
            Ok(prepared) => prepared,
            Err(failure) => {
                show_message(
                    &format!("创建置顶贴图失败，当前截图和标注已保留。\n\n{failure}"),
                    true,
                );
                return;
            }
        };
        let Some(operation) = self.capture_operation.take() else {
            return;
        };
        let out = match operation.commit_pin(plan) {
            Ok(image) => image,
            Err((operation, _)) => {
                self.capture_operation = Some(operation);
                return;
            }
        };
        let pin = prepared.finish(out);
        let id = pin.window_id();
        self.set_pins_visible(true);
        if let Some(replaced) = self.pins.insert(id, pin) {
            replaced.close();
        }
        if let Some(pin) = self.pins.get(&id) {
            pin.request_redraw();
        }
    }
}
impl App {
    fn take_capture_failure_notice(&mut self, stage: CaptureFailureStage) -> Option<String> {
        if !self
            .capture_operation
            .as_ref()
            .is_some_and(CaptureOperation::is_preparing)
        {
            return None;
        }
        let Some(operation) = self.capture_operation.take() else {
            return None;
        };
        let CaptureEnd::CaptureFailed(stage) = operation.capture_failed(stage);
        self.close_overlay();
        Some(format!("无法开始截图，请重试。\n错误码：{}", stage.code()))
    }

    fn handle_capture_failure(&mut self, stage: CaptureFailureStage) {
        let Some(message) = self.take_capture_failure_notice(stage) else {
            return;
        };
        if self.diagnostics_enabled && record_capture_failure(stage).is_err() {
            // 诊断失败本身不能阻止恢复；这里只输出固定字段，不泄露路径或系统错误文本。
            eprintln!(
                "event=capture_diagnostic_write_failed code={}",
                stage.code()
            );
        }
        show_message(&message, true);
    }

    /// 清理失败会话并返回是否真的存在活动资源；用于屏蔽旧事件造成的重复报错。
    fn recover_failed_session(&mut self) -> bool {
        let was_active = self.capture_operation.is_some();
        self.close_overlay();
        was_active
    }

    fn take_session_failure_notice(&mut self, failure: SessionFailure) -> Option<String> {
        if !self.recover_failed_session() {
            return None;
        }
        Some(format!(
            "截图会话发生错误，已安全关闭。\n\n{failure}\n\n请重新按截图快捷键重试。"
        ))
    }

    fn handle_session_failure(&mut self, failure: SessionFailure) {
        if let Some(message) = self.take_session_failure_notice(failure) {
            show_message(&message, true);
        }
    }

    fn handle_pin_failure(&mut self, id: WindowId, failure: SessionFailure) {
        if self.close_pin(id) {
            show_message(
                &format!("一张置顶贴图发生错误，已单独关闭。\n\n{failure}"),
                true,
            );
        }
    }

    fn close_pin(&mut self, id: WindowId) -> bool {
        let Some(pin) = self.pins.remove(&id) else {
            return false;
        };
        pin.close();
        true
    }

    fn set_pins_visible(&mut self, visible: bool) {
        if self.pins.is_empty() {
            return;
        }
        for pin in self.pins.values_mut() {
            if !visible {
                pin.end_drag();
            }
            pin.set_visible(visible);
        }
        flush_window_compositor();
    }

    fn cleanup_temp_files_if_due(&mut self, now: Instant) {
        if !claim_temp_cleanup_slot(&mut self.last_temp_cleanup, now) {
            return;
        }
        if let Err(error) = std::thread::Builder::new()
            .name(String::from("rshot-temp-cleanup"))
            .spawn(|| {
                if let Err(error) = cleanup_expired_temp_pngs(SystemTime::now()) {
                    eprintln!("清理 rshot 临时图片失败：{error}");
                }
            })
        {
            eprintln!("无法启动 rshot 临时图片清理线程：{error}");
        }
    }

    /// 关掉活动截图窗口，回后台待命；已有贴图恢复显示且不被销毁。
    fn close_overlay(&mut self) {
        if let Some(operation) = self.capture_operation.take() {
            operation.close();
        }
        self.set_pins_visible(true);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Annotation, App, CaptureFailureStage, CaptureOperation, CapturePhase, Config,
        MAX_PINNED_WINDOWS, OcrBackend, OcrCharacterData, OcrFallbackReason, OcrLineData,
        OcrRegionData, OcrWordData, PALETTE, PublishedImageFormats, SessionFailure,
        SessionFailureStage, Shape, TEMP_PNG_CLEANUP_INTERVAL, TEMP_PNG_MAX_AGE, TEXT_FONT_HEIGHT,
        TOOLBAR_SLOT_COLOR, TOOLBAR_SLOT_COUNT, Tool, ToolbarAction, ToolbarItem, blit_rgba_image,
        build_about_message, build_dib, capture_failure_log_line, choose_ocr_backend,
        claim_temp_cleanup_slot, cleanup_expired_temp_pngs_in, color_u32, crop_image,
        dragged_window_position, draw_annotation_image, draw_line_image, draw_rect_image,
        embedded_character_count, gdi_text_size, has_pin_capacity,
        image_clipboard_publish_succeeded, is_cjk_language_tag, is_managed_temp_png,
        normalized_rect, ocr_region, palette_hit, palette_popup_rect, palette_swatch_rect,
        prepare_ocr_rgba, prepare_ocr_rgba_for_recognition, prepare_ocr_worker_rgba,
        rebuild_model_ocr_text, rebuild_ocr_text, record_capture_failure_in, regroup_ocr_lines,
        restore_model_cross_region_spacing, restore_model_region_spacing, toolbar_hit,
        toolbar_item, toolbar_item_rect, toolbar_item_slot, toolbar_origin, toolbar_size,
        unicode_text_bytes, worker_protocol_round_trip, write_unique_temp_png_in,
    };
    use std::collections::HashSet;
    use std::fs::{self, FileTimes, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use xcap::image::{Rgb, RgbImage, RgbaImage};

    static TEST_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn install_test_capture(app: &mut App, image: RgbaImage) {
        app.capture_operation =
            Some(CaptureOperation::begin().capture_succeeded_without_window(image));
    }

    #[test]
    fn embedded_ppocrv6_dictionary_matches_the_recognition_model() {
        assert_eq!(embedded_character_count(), 18_708);
    }

    #[test]
    fn ocr_backend_reports_model_success_without_calling_fallback() {
        let result = choose_ocr_backend(Ok(String::from("  text  ")), || {
            panic!("Windows OCR should not run after model success")
        })
        .unwrap();
        assert_eq!(result.text, "text");
        assert_eq!(result.backend, OcrBackend::PpOcrV6);
        assert_eq!(result.fallback_reason, None);
    }

    #[test]
    fn ocr_backend_explicitly_reports_windows_fallback() {
        let result = choose_ocr_backend(Err(String::from("model unavailable")), || {
            Ok(String::from("fallback text"))
        })
        .unwrap();
        assert_eq!(result.text, "fallback text");
        assert_eq!(result.backend, OcrBackend::Windows);
        assert_eq!(
            result.fallback_reason,
            Some(OcrFallbackReason::ModelUnavailable)
        );

        let empty_model =
            choose_ocr_backend(Ok(String::new()), || Ok(String::from("fallback text"))).unwrap();
        assert_eq!(
            empty_model.fallback_reason,
            Some(OcrFallbackReason::ModelReturnedNoText)
        );

        let error = choose_ocr_backend(Err(String::from("model unavailable")), || {
            Err(String::from("language pack unavailable"))
        })
        .unwrap_err();
        assert!(error.contains("model unavailable"));
        assert!(error.contains("language pack unavailable"));
    }

    #[test]
    fn about_message_shows_author_and_loaded_hotkeys() {
        let message = build_about_message("Ctrl+Shift+S", "Ctrl+Shift+Q", true);

        assert!(message.contains("idkwhatimdoing62"));
        assert!(message.contains("Ctrl+Shift+S"));
        assert!(message.contains("Ctrl+Shift+Q"));
        assert!(message.contains("诊断日志：开启"));
        assert!(message.contains("截图只能通过全局热键触发"));
    }

    #[test]
    fn existing_config_without_diagnostics_keeps_working() {
        let dir = TestTempDir::new();
        let path = dir.path().join("config.yml");
        fs::write(&path, "hotkey: Alt+A\nquit: Alt+D\n").unwrap();

        let config: Config = confy::load_path(&path).unwrap();

        assert_eq!(config.hotkey, "Alt+A");
        assert_eq!(config.quit, "Alt+D");
        assert!(config.diagnostics);
    }

    #[test]
    fn pin_capacity_accepts_eight_but_rejects_the_ninth() {
        assert!(has_pin_capacity(0));
        assert!(has_pin_capacity(MAX_PINNED_WINDOWS - 1));
        assert!(!has_pin_capacity(MAX_PINNED_WINDOWS));
        assert!(!has_pin_capacity(MAX_PINNED_WINDOWS + 1));
    }

    #[test]
    fn pinned_window_drag_preserves_the_initial_pointer_offset() {
        assert_eq!(
            dragged_window_position((100, 80), (400, 300), (125, 110)),
            (425, 330)
        );
    }

    #[test]
    fn rgba_blit_copies_rows_and_clears_unused_pin_area() {
        let image = RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255])
            .expect("valid test image");
        let mut buffer = vec![u32::MAX; 6];

        blit_rgba_image(&mut buffer, 3, 2, &image);

        assert_eq!(buffer, vec![0x00FF0000, 0x0000FF00, 0, 0, 0, 0]);
    }

    struct TestTempDir {
        root: PathBuf,
        path: PathBuf,
    }

    impl TestTempDir {
        fn new() -> Self {
            let root = std::env::temp_dir();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let sequence = TEST_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = root.join(format!(
                "rshot-test-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create isolated test temp directory");
            Self { root, path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            let safe_name = self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rshot-test-"));
            if self.path.parent() == Some(self.root.as_path()) && safe_name {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    fn create_file_with_mtime(path: &Path, modified: SystemTime) {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .expect("create test file");
        file.write_all(b"test").expect("write test file");
        file.set_times(FileTimes::new().set_modified(modified))
            .expect("set test mtime");
    }

    #[test]
    fn dib_header_and_bgr() {
        // 2×1，一红一绿；stride = (2*3+3)&!3 = 8，总长 40+8 = 48
        let img = RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 255]).unwrap();
        let d = build_dib(&img);
        assert_eq!(d.len(), 48);
        assert_eq!(&d[0..4], &40u32.to_le_bytes()); // biSize
        assert_eq!(&d[4..8], &2i32.to_le_bytes()); // biWidth
        assert_eq!(d[14], 24); // biBitCount 低字节
        // 像素段：红 → B,G,R = 0,0,255；绿 → 0,255,0
        assert_eq!(&d[40..46], &[0, 0, 255, 0, 255, 0]);
    }

    #[test]
    fn image_copy_closes_only_after_a_format_is_published_and_clipboard_is_closed() {
        let none = PublishedImageFormats::new(false, false);
        let dib_only = PublishedImageFormats::new(true, false);
        let file_only = PublishedImageFormats::new(false, true);
        let both = PublishedImageFormats::new(true, true);

        assert!(!image_clipboard_publish_succeeded(none, true));
        assert!(image_clipboard_publish_succeeded(dib_only, true));
        assert!(image_clipboard_publish_succeeded(file_only, true));
        assert!(image_clipboard_publish_succeeded(both, true));
        assert!(!image_clipboard_publish_succeeded(dib_only, false));
        assert!(!image_clipboard_publish_succeeded(file_only, false));
        assert!(!image_clipboard_publish_succeeded(both, false));
        assert!(dib_only.dib());
        assert!(!dib_only.file());
        assert!(!file_only.dib());
        assert!(file_only.file());
        assert_eq!(none.description(), "无");
        assert_eq!(dib_only.description(), "位图（CF_DIB）");
        assert_eq!(file_only.description(), "PNG 文件（CF_HDROP）");
        assert_eq!(both.description(), "位图（CF_DIB）和 PNG 文件（CF_HDROP）");
    }

    #[test]
    fn crop_accepts_reverse_drag_and_clamps_to_image() {
        let img = RgbaImage::new(10, 8);
        let cropped = crop_image(&img, (9, 7), (-3, 2)).unwrap();
        assert_eq!(cropped.dimensions(), (9, 5));
        assert_eq!(normalized_rect(((9, 7), (-3, 2))), (-3, 2, 9, 7));
        assert!(crop_image(&img, (1, 3), (8, 3)).is_none());
        assert!(crop_image(&img, (4, 1), (4, 7)).is_none());
    }

    #[test]
    fn pen_draws_into_output_image() {
        let mut img = RgbaImage::new(20, 20);
        draw_line_image(&mut img, (2, 2), (17, 17), [255, 45, 45, 255], 2);
        assert_eq!(img.get_pixel(10, 10).0, [255, 45, 45, 255]);
        assert_eq!(img.get_pixel(19, 0).0, [0, 0, 0, 0]);
    }

    #[test]
    fn toolbar_hit_targets_each_slot() {
        let sel = Some(((300, 200), (600, 500)));
        let origin = toolbar_origin(1920, 1080, sel);
        for slot in 0..TOOLBAR_SLOT_COUNT {
            let rect = toolbar_item_rect(origin, slot);
            let mid = (
                rect.0 + (rect.2 - rect.0) / 2,
                rect.1 + (rect.3 - rect.1) / 2,
            );
            assert_eq!(
                toolbar_hit(mid, 1920, 1080, sel),
                Some(toolbar_item(slot)),
                "slot {slot}"
            );
            assert_eq!(toolbar_item_slot(toolbar_item(slot)), slot);
        }
    }

    #[test]
    fn toolbar_background_ends_at_last_item() {
        let origin = (40, 60);
        let (width, height) = toolbar_size();
        let last = toolbar_item_rect(origin, TOOLBAR_SLOT_COUNT - 1);
        assert_eq!(origin.0 + width, last.2);
        assert_eq!(origin.1 + height, last.3);
    }

    #[test]
    fn toolbar_slot_layout_matches_expectation() {
        // 单行：4 个工具，COLOR 按钮，6 个动作
        assert_eq!(toolbar_item(0), ToolbarItem::Tool(Tool::Pen));
        assert_eq!(toolbar_item(1), ToolbarItem::Tool(Tool::Line));
        assert_eq!(toolbar_item(2), ToolbarItem::Tool(Tool::Rect));
        assert_eq!(toolbar_item(3), ToolbarItem::Tool(Tool::Text));
        assert_eq!(toolbar_item(4), ToolbarItem::Color);
        assert_eq!(toolbar_item(5), ToolbarItem::Action(ToolbarAction::Undo));
        assert_eq!(toolbar_item(7), ToolbarItem::Action(ToolbarAction::Ocr));
        assert_eq!(toolbar_item(10), ToolbarItem::Action(ToolbarAction::Close));
        assert_eq!(toolbar_item_slot(ToolbarItem::Color), 4);
        assert_eq!(
            toolbar_item_slot(ToolbarItem::Action(ToolbarAction::Ocr)),
            7
        );
    }

    #[test]
    fn toolbar_fits_a_640_pixel_wide_screen() {
        assert!(toolbar_size().0 <= 640 - 16);
    }

    #[test]
    fn ocr_uses_original_selected_rgba_pixels() {
        let img = RgbaImage::from_fn(4, 3, |x, y| {
            xcap::image::Rgba([(x * 20) as u8, (y * 30) as u8, 7, 255])
        });
        let (pixels, width, height) = prepare_ocr_rgba(&img, Some(((3, 2), (1, 0))), 100).unwrap();
        assert_eq!((width, height), (2, 2));
        assert_eq!(
            pixels.as_ref(),
            &[20, 0, 7, 255, 40, 0, 7, 255, 20, 30, 7, 255, 40, 30, 7, 255,]
        );
        assert_eq!(
            ocr_region(&img, Some(((8, 9), (-2, 1)))),
            Some((0, 1, 4, 2))
        );
    }

    #[test]
    fn ocr_input_scales_to_the_system_dimension_limit() {
        let img = RgbaImage::new(8, 4);
        let (pixels, width, height) = prepare_ocr_rgba(&img, None, 4).unwrap();
        assert_eq!((width, height), (4, 2));
        assert_eq!(pixels.len(), 4 * 2 * 4);
    }

    #[test]
    fn full_image_ocr_input_reuses_the_original_buffer() {
        let img = RgbaImage::new(8, 4);
        let (pixels, width, height) = prepare_ocr_rgba(&img, None, 100).unwrap();
        assert_eq!((width, height), img.dimensions());
        assert!(matches!(pixels, std::borrow::Cow::Borrowed(_)));
    }

    fn ocr_word(text: &str, x: f32, width: f32, height: f32) -> OcrWordData {
        OcrWordData {
            text: text.to_owned(),
            x,
            y: 0.0,
            width,
            height,
        }
    }

    fn ocr_word_at(text: &str, x: f32, y: f32, width: f32, height: f32) -> OcrWordData {
        OcrWordData {
            text: text.to_owned(),
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn small_ocr_input_is_upscaled_without_exceeding_the_system_limit() {
        let img = RgbaImage::new(400, 200);
        let (pixels, width, height) = prepare_ocr_rgba_for_recognition(&img, None, 10_000).unwrap();
        assert_eq!((width, height), (800, 400));
        assert!(matches!(pixels, std::borrow::Cow::Owned(_)));

        let (pixels, width, height) = prepare_ocr_rgba_for_recognition(&img, None, 600).unwrap();
        assert_eq!((width, height), img.dimensions());
        assert!(matches!(pixels, std::borrow::Cow::Borrowed(_)));

        let wide = RgbaImage::new(1100, 500);
        let (pixels, width, height) =
            prepare_ocr_rgba_for_recognition(&wide, None, 10_000).unwrap();
        assert_eq!((width, height), wide.dimensions());
        assert!(matches!(pixels, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn ocr_text_rebuild_removes_fake_chinese_spaces_and_preserves_lines() {
        let lines = vec![
            OcrLineData {
                words: vec![
                    ocr_word("·", 0.0, 7.0, 7.0),
                    ocr_word("严", 24.0, 19.0, 19.0),
                    ocr_word("格", 44.0, 19.0, 19.0),
                    ocr_word("采", 64.0, 19.0, 19.0),
                    ocr_word("用", 84.0, 19.0, 19.0),
                    ocr_word("；", 105.0, 4.0, 16.0),
                ],
            },
            OcrLineData {
                words: vec![
                    ocr_word("补", 24.0, 19.0, 19.0),
                    ocr_word("齐", 44.0, 19.0, 19.0),
                    ocr_word("HTDP", 70.0, 47.0, 19.0),
                    ocr_word("数", 124.0, 19.0, 19.0),
                    ocr_word("据", 144.0, 19.0, 19.0),
                    ocr_word("形", 164.0, 19.0, 19.0),
                    ocr_word("状", 184.0, 19.0, 19.0),
                    ocr_word("、", 204.0, 6.0, 19.0),
                    ocr_word("操", 224.0, 19.0, 19.0),
                    ocr_word("作", 244.0, 19.0, 19.0),
                ],
            },
        ];

        assert_eq!(
            rebuild_ocr_text(&lines),
            "• 严格采用；\r\n补齐 HTDP 数据形状、操作"
        );
    }

    #[test]
    fn ocr_text_rebuild_keeps_real_latin_spaces_and_does_not_guess_characters() {
        let lines = vec![
            OcrLineData {
                words: vec![
                    ocr_word("hello", 0.0, 40.0, 20.0),
                    ocr_word("world", 43.0, 42.0, 20.0),
                ],
            },
            OcrLineData {
                words: vec![
                    ocr_word("第", 0.0, 19.0, 20.0),
                    ocr_word("3", 20.0, 10.0, 20.0),
                    ocr_word("章", 31.0, 19.0, 20.0),
                    ocr_word("Context", 58.0, 70.0, 20.0),
                    ocr_word("、", 129.0, 6.0, 20.0),
                    ocr_word("Container", 149.0, 84.0, 20.0),
                ],
            },
            OcrLineData {
                words: vec![
                    ocr_word("窗", 0.0, 19.0, 20.0),
                    ocr_word("囗", 20.0, 19.0, 20.0),
                ],
            },
        ];

        assert_eq!(
            rebuild_ocr_text(&lines),
            "hello world\r\n第3章 Context、Container\r\n窗囗"
        );
    }

    #[test]
    fn ocr_text_rebuild_repairs_only_flat_dashes_inside_ascii_identifiers() {
        let first_dash = ocr_word("一", 13.0, 6.0, 2.0);
        let second_dash = ocr_word("·", 71.0, 6.0, 2.0);
        let lines = vec![
            OcrLineData {
                words: vec![
                    ocr_word("D", 0.0, 12.0, 14.0),
                    first_dash,
                    ocr_word("01", 20.0, 18.0, 14.0),
                    ocr_word("～", 39.0, 18.0, 6.0),
                    ocr_word("D", 58.0, 12.0, 14.0),
                    second_dash,
                    ocr_word("13", 78.0, 18.0, 14.0),
                    ocr_word("决", 103.0, 19.0, 20.0),
                    ocr_word("策", 123.0, 19.0, 20.0),
                ],
            },
            OcrLineData {
                words: vec![
                    ocr_word("决", 0.0, 19.0, 20.0),
                    ocr_word("策", 20.0, 19.0, 20.0),
                    ocr_word("记", 40.0, 19.0, 20.0),
                    ocr_word("录", 60.0, 19.0, 20.0),
                ],
            },
            OcrLineData {
                words: vec![
                    ocr_word("第", 0.0, 19.0, 20.0),
                    ocr_word("一", 20.0, 19.0, 20.0),
                    ocr_word("章", 40.0, 19.0, 20.0),
                ],
            },
            OcrLineData {
                words: vec![
                    ocr_word("API", 0.0, 24.0, 20.0),
                    ocr_word("一", 36.0, 6.0, 2.0),
                    ocr_word("01", 54.0, 18.0, 20.0),
                ],
            },
            OcrLineData {
                words: vec![
                    ocr_word("UTF", 0.0, 30.0, 20.0),
                    ocr_word("-", 31.0, 6.0, 20.0),
                    ocr_word("8", 38.0, 10.0, 20.0),
                    ocr_word("编", 49.0, 19.0, 20.0),
                    ocr_word("码", 69.0, 19.0, 20.0),
                ],
            },
        ];

        assert_eq!(
            rebuild_ocr_text(&lines),
            "D-01～D-13 决策\r\n决策记录\r\n第一章\r\nAPI 一 01\r\nUTF-8编码"
        );
    }

    #[test]
    fn ocr_layout_rebuild_is_limited_to_cjk_recognizer_languages() {
        assert!(is_cjk_language_tag("zh-Hans-CN"));
        assert!(is_cjk_language_tag("ja-JP"));
        assert!(is_cjk_language_tag("ko_KR"));
        assert!(!is_cjk_language_tag("en-US"));
        assert!(!is_cjk_language_tag("fr-FR"));
    }

    #[test]
    fn windows_ocr_fragments_are_reordered_by_physical_coordinates() {
        let lines = vec![
            OcrLineData {
                words: vec![ocr_word_at("• 示例图", 43.0, 149.0, 357.0, 22.0)],
            },
            OcrLineData {
                words: vec![ocr_word_at("下一行", 43.0, 195.0, 70.0, 20.0)],
            },
            OcrLineData {
                words: vec![ocr_word_at("Container、", 397.0, 148.0, 202.0, 25.0)],
            },
            OcrLineData {
                words: vec![ocr_word_at("窗口、", 609.0, 148.0, 79.0, 26.0)],
            },
            OcrLineData {
                words: vec![ocr_word_at("剪贴板均正确。", 677.0, 148.0, 151.0, 25.0)],
            },
        ];

        let rebuilt = regroup_ocr_lines(&lines);
        assert_eq!(rebuilt.len(), 2);
        assert_eq!(
            rebuilt[0]
                .words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            vec!["• 示例图", "Container、", "窗口、", "剪贴板均正确。"]
        );
        assert_eq!(rebuilt[1].words[0].text, "下一行");
    }

    #[test]
    fn model_ocr_rebuild_merges_gray_code_spans_and_preserves_mixed_text() {
        let region = |text: &str, x: f32, y: f32, width: f32, height: f32| OcrRegionData {
            text: text.to_owned(),
            x,
            y,
            width,
            height,
            space_before: false,
        };
        let regions = vec![
            region(
                "• 小图在 200 万像素预算内放大 2 倍，提高小字和标点准确率。",
                43.0,
                22.0,
                547.0,
                21.0,
            ),
            region(
                "• 中日韩 OCR 按”行、词、坐标”重建，保留 7 行及项目符号。",
                43.0,
                64.0,
                530.0,
                21.0,
            ),
            region(
                "• 保护英文空格、UTF-8编码和正文中的“一”，避免后处理误改。",
                42.0,
                103.0,
                569.0,
                28.0,
            ),
            region(
                "● 示例图已端到端验证，D-01~D-13、",
                43.0,
                149.0,
                357.0,
                22.0,
            ),
            region("Context、Container、", 397.0, 148.0, 202.0, 25.0),
            region("窗口、", 609.0, 148.0, 79.0, 26.0),
            region("剪贴板均正确。", 677.0, 148.0, 151.0, 25.0),
            region(
                "• 49 项测试全部通过，Release 构建成功。",
                44.0,
                195.0,
                363.0,
                20.0,
            ),
        ];

        assert_eq!(
            rebuild_model_ocr_text(&regions),
            concat!(
                "• 小图在 200 万像素预算内放大 2 倍，提高小字和标点准确率。\r\n",
                "• 中日韩 OCR 按“行、词、坐标”重建，保留 7 行及项目符号。\r\n",
                "• 保护英文空格、UTF-8编码和正文中的“一”，避免后处理误改。\r\n",
                "• 示例图已端到端验证，D-01~D-13、Context、Container、窗口、剪贴板均正确。\r\n",
                "• 49 项测试全部通过，Release 构建成功。"
            )
        );
    }

    #[test]
    fn model_ocr_input_and_worker_protocol_are_bounded_and_lossless() {
        let image = RgbaImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        let (rgba, width, height) = prepare_ocr_worker_rgba(&image, None).unwrap();
        let decoded = worker_protocol_round_trip(&rgba, width, height).unwrap();
        assert_eq!(decoded, image);

        let large = RgbaImage::new(5000, 2000);
        let (rgba, width, height) = prepare_ocr_worker_rgba(&large, None).unwrap();
        assert!(width <= 4096);
        assert!(width as u64 * height as u64 <= 8_000_000);
        assert_eq!(rgba.len(), width as usize * height as usize * 4);
    }

    #[test]
    fn model_ocr_spacing_keeps_ordinals_and_mixed_identifiers_intact() {
        let regions = vec![OcrRegionData {
            text: String::from("第3章使用UTF-8编码和4K屏，含49项"),
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 20.0,
            space_before: false,
        }];
        assert_eq!(
            rebuild_model_ocr_text(&regions),
            "第3章使用UTF-8编码和4K屏，含49项"
        );
    }

    fn model_spacing_fixture(
        text: &str,
        boundary_after: usize,
        blank_columns: u32,
        background: [u8; 3],
    ) -> (RgbImage, Vec<OcrCharacterData>) {
        let chars: Vec<char> = text.chars().collect();
        let mut positions = Vec::with_capacity(chars.len());
        let mut cursor = 4_u32;
        for (index, ch) in chars.iter().copied().enumerate() {
            positions.push((ch, cursor));
            cursor += 6;
            if index + 1 < chars.len() {
                cursor += if index == boundary_after {
                    blank_columns
                } else {
                    1
                };
            }
        }
        let mut image = RgbImage::from_pixel(cursor + 4, 28, Rgb(background));
        let mut characters = Vec::with_capacity(chars.len());
        for (ch, x) in positions {
            for pixel_y in 5..22 {
                for pixel_x in x..x + 6 {
                    if pixel_x == x || pixel_x == x + 5 || pixel_y == 5 || pixel_y == 21 {
                        image.put_pixel(pixel_x, pixel_y, Rgb([20, 20, 20]));
                    }
                }
            }
            characters.push(OcrCharacterData {
                ch,
                x: x.saturating_sub(1) as f32,
                y: 3.0,
                width: 8.0,
                height: 21.0,
            });
        }
        (image, characters)
    }

    #[test]
    fn model_ocr_restores_only_pixel_verified_mixed_text_spaces() {
        for (text, boundary, gap, expected) in [
            ("2倍", 0, 6, "2 倍"),
            ("图2", 0, 6, "图 2"),
            ("OCR按", 2, 6, "OCR 按"),
            ("49项", 1, 6, "49 项"),
            ("苹果iPhone", 1, 1, "苹果iPhone"),
            ("Windows窗口", 6, 1, "Windows窗口"),
            ("第3章", 0, 1, "第3章"),
            ("4K屏", 1, 1, "4K屏"),
        ] {
            let (image, characters) = model_spacing_fixture(text, boundary, gap, [255, 255, 255]);
            assert_eq!(
                restore_model_region_spacing(text, &characters, &image),
                expected,
                "case: {text}"
            );
        }

        let (image, characters) = model_spacing_fixture("UTF-8编码", 4, 1, [238, 238, 238]);
        assert_eq!(
            restore_model_region_spacing("UTF-8编码", &characters, &image),
            "UTF-8编码"
        );
        assert_eq!(
            restore_model_region_spacing("框数不匹配", &characters[..2], &image),
            "框数不匹配"
        );
    }

    #[test]
    fn model_ocr_ignores_wide_blank_inside_cjk_glyphs() {
        let width = 46;
        let mut image = RgbImage::from_pixel(width, 28, Rgb([255, 255, 255]));
        // 模拟“川”一类内部存在宽竖向空带的字形；两字符真实边界只有 1px。
        for x in [4..7, 13..16, 22..25] {
            for pixel_x in x {
                for pixel_y in 5..22 {
                    image.put_pixel(pixel_x, pixel_y, Rgb([20, 20, 20]));
                }
            }
        }
        for pixel_x in 26..34 {
            for pixel_y in 5..22 {
                if pixel_x == 26 || pixel_x == 33 || pixel_y == 5 || pixel_y == 21 {
                    image.put_pixel(pixel_x, pixel_y, Rgb([20, 20, 20]));
                }
            }
        }
        let characters = vec![
            OcrCharacterData {
                ch: '川',
                x: 3.0,
                y: 3.0,
                width: 22.0,
                height: 21.0,
            },
            OcrCharacterData {
                ch: 'A',
                x: 25.0,
                y: 3.0,
                width: 10.0,
                height: 21.0,
            },
        ];
        assert_eq!(
            restore_model_region_spacing("川A", &characters, &image),
            "川A"
        );
        let undercovered = vec![
            OcrCharacterData {
                width: 14.0,
                ..characters[0].clone()
            },
            characters[1].clone(),
        ];
        assert_eq!(
            restore_model_region_spacing("川A", &undercovered, &image),
            "川A"
        );

        let mut image = RgbImage::from_pixel(width, 28, Rgb([255, 255, 255]));
        for pixel_x in 4..12 {
            for pixel_y in 5..22 {
                if pixel_x == 4 || pixel_x == 11 || pixel_y == 5 || pixel_y == 21 {
                    image.put_pixel(pixel_x, pixel_y, Rgb([20, 20, 20]));
                }
            }
        }
        for x in [13..16, 22..25, 31..34] {
            for pixel_x in x {
                for pixel_y in 5..22 {
                    image.put_pixel(pixel_x, pixel_y, Rgb([20, 20, 20]));
                }
            }
        }
        let characters = vec![
            OcrCharacterData {
                ch: 'A',
                x: 3.0,
                y: 3.0,
                width: 10.0,
                height: 21.0,
            },
            OcrCharacterData {
                ch: '川',
                x: 12.0,
                y: 3.0,
                width: 23.0,
                height: 21.0,
            },
        ];
        assert_eq!(
            restore_model_region_spacing("A川", &characters, &image),
            "A川"
        );
        let undercovered = vec![
            characters[0].clone(),
            OcrCharacterData {
                x: 20.0,
                width: 15.0,
                ..characters[1].clone()
            },
        ];
        assert_eq!(
            restore_model_region_spacing("A川", &undercovered, &image),
            "A川"
        );
    }

    #[test]
    fn model_ocr_cross_region_spaces_also_require_pixel_evidence() {
        let (image, characters) = model_spacing_fixture("OCR按", 2, 6, [255, 255, 255]);
        let mut regions = vec![
            OcrRegionData {
                text: String::from("OCR"),
                x: characters[0].x,
                y: 3.0,
                width: characters[2].x + characters[2].width - characters[0].x,
                height: 21.0,
                space_before: false,
            },
            OcrRegionData {
                text: String::from("按"),
                x: characters[3].x,
                y: 3.0,
                width: characters[3].width,
                height: 21.0,
                space_before: false,
            },
        ];
        let character_groups = vec![characters[..3].to_vec(), characters[3..].to_vec()];
        restore_model_cross_region_spacing(&mut regions, &character_groups, &image);
        assert!(regions[1].space_before);
        assert_eq!(rebuild_model_ocr_text(&regions), "OCR 按");
    }

    #[test]
    fn model_ocr_does_not_merge_distant_same_row_columns() {
        let regions = vec![
            OcrRegionData {
                text: String::from("左栏"),
                x: 10.0,
                y: 20.0,
                width: 40.0,
                height: 20.0,
                space_before: false,
            },
            OcrRegionData {
                text: String::from("右栏"),
                x: 300.0,
                y: 20.0,
                width: 40.0,
                height: 20.0,
                space_before: false,
            },
        ];
        assert_eq!(rebuild_model_ocr_text(&regions), "左栏\r\n右栏");
    }

    #[test]
    fn unicode_clipboard_text_round_trips_chinese_and_emoji() {
        let bytes = unicode_text_bytes("中文😀\nabc");
        assert_eq!(&bytes[bytes.len() - 2..], &[0, 0]);
        let utf16: Vec<u16> = bytes[..bytes.len() - 2]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        assert_eq!(String::from_utf16(&utf16).unwrap(), "中文😀\nabc");
    }

    #[test]
    fn palette_hit_targets_each_swatch() {
        let sel = Some(((300, 200), (600, 500)));
        let origin = toolbar_origin(1920, 1080, sel);
        let color_rect = toolbar_item_rect(origin, TOOLBAR_SLOT_COLOR);
        let popup = palette_popup_rect(1920, 1080, color_rect);
        for i in 0..PALETTE.len() {
            let (x0, y0, x1, y1) = palette_swatch_rect(popup, i);
            let mid = ((x0 + x1) / 2, (y0 + y1) / 2);
            assert_eq!(palette_hit(mid, 1920, 1080, sel), Some(i), "swatch {i}");
        }
        // 工具栏本体上的点不该被当成色块
        assert_eq!(
            palette_hit((origin.0 + 10, origin.1 + 10), 1920, 1080, sel),
            None
        );
    }

    #[test]
    fn palette_colors_are_distinct_rgb() {
        let mut seen = std::collections::HashSet::new();
        for c in PALETTE {
            assert!(seen.insert(color_u32(c)), "duplicate palette color {c:?}");
        }
    }

    #[test]
    fn rect_annotation_draws_border_only() {
        let mut img = RgbaImage::new(20, 20);
        draw_rect_image(&mut img, (4, 4), (14, 14), [255, 0, 0, 255], 3);
        assert_eq!(img.get_pixel(9, 4).0, [255, 0, 0, 255]); // 上边线
        assert_eq!(img.get_pixel(5, 5).0, [255, 0, 0, 255]); // 左边线
        assert_eq!(img.get_pixel(9, 9).0, [0, 0, 0, 0]); // 内部透明
    }

    #[test]
    fn line_annotation_uses_its_color() {
        let mut img = RgbaImage::new(20, 20);
        let ann = Annotation {
            shape: Shape::Line((2, 2), (17, 17)),
            color: [0, 200, 0, 255],
        };
        draw_annotation_image(&mut img, &ann, (0, 0));
        assert_eq!(img.get_pixel(10, 10).0, [0, 200, 0, 255]);
    }

    #[test]
    fn annotation_image_respects_selection_offset() {
        let mut img = RgbaImage::new(20, 20);
        let ann = Annotation {
            shape: Shape::Rect((10, 10), (14, 14)),
            color: [0, 0, 255, 255],
        };
        draw_annotation_image(&mut img, &ann, (8, 8));
        assert_eq!(img.get_pixel(5, 5).0, [0, 0, 255, 255]); // 10-8=2 处的边框→画在(2..6)
    }

    #[test]
    fn text_annotation_draws_into_image() {
        // GDI 文字渲染在 CI 环境可能没有字体，此处只验证能端到端跑通不 panic
        let mut img = RgbaImage::new(200, 60);
        let ann = Annotation {
            shape: Shape::Text((10, 10), String::from("Ab1 测试")),
            color: [255, 0, 0, 255],
        };
        draw_annotation_image(&mut img, &ann, (0, 0));
        let _ = img;
    }

    #[test]
    fn empty_text_does_not_reach_drawtext() {
        // 空文字走 gdi_text_size 必须直接返回最小尺寸，绝不能把空切片交给 DrawTextW（会读越界闪退）
        let (w, h) = gdi_text_size("");
        assert_eq!(w, 1);
        assert_eq!(h, TEXT_FONT_HEIGHT);
    }

    #[test]
    fn capture_failures_use_stable_privacy_safe_codes() {
        let stages = [
            (CaptureFailureStage::ReadCursor, "RSH-CAP-001"),
            (CaptureFailureStage::LocateCaptureMonitor, "RSH-CAP-002"),
            (CaptureFailureStage::MatchOverlayMonitor, "RSH-CAP-003"),
            (CaptureFailureStage::CaptureImage, "RSH-CAP-004"),
        ];
        for (stage, code) in stages {
            assert_eq!(stage.code(), code);
        }

        let line = capture_failure_log_line(
            CaptureFailureStage::CaptureImage,
            UNIX_EPOCH + Duration::from_secs(123),
        );
        assert_eq!(
            line,
            "unix_seconds=123 event=capture_failed code=RSH-CAP-004\n"
        );
        assert!(!line.contains("cursor"));
        assert!(!line.contains("monitor"));
        assert!(!line.contains("window"));
    }

    #[test]
    fn capture_diagnostic_log_stops_at_its_size_limit() {
        let dir = TestTempDir::new();
        let path = dir.path().join("capture-errors.log");
        let now = UNIX_EPOCH + Duration::from_secs(456);
        let line = capture_failure_log_line(CaptureFailureStage::ReadCursor, now);
        let max_bytes = (line.len() * 2) as u64;

        assert!(
            record_capture_failure_in(&path, CaptureFailureStage::ReadCursor, now, max_bytes,)
                .unwrap()
        );
        assert!(
            record_capture_failure_in(&path, CaptureFailureStage::ReadCursor, now, max_bytes,)
                .unwrap()
        );
        assert!(
            !record_capture_failure_in(&path, CaptureFailureStage::ReadCursor, now, max_bytes,)
                .unwrap()
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), max_bytes);
    }

    #[test]
    fn successful_capture_transition_finishes_the_entry_attempt() {
        let mut app = App::default();

        app.begin_capture_attempt();
        assert_eq!(
            app.capture_operation.as_ref().map(CaptureOperation::phase),
            Some(CapturePhase::Preparing)
        );
        let operation = app.capture_operation.take().unwrap();
        app.capture_operation =
            Some(operation.capture_succeeded_without_window(RgbaImage::new(2, 2)));

        assert_eq!(
            app.capture_operation.as_ref().map(CaptureOperation::phase),
            Some(CapturePhase::Selecting)
        );
        assert!(
            app.take_capture_failure_notice(CaptureFailureStage::CaptureImage)
                .is_none()
        );
    }

    #[test]
    fn repeated_capture_failures_leave_no_old_session_and_keep_hotkeys() {
        let mut app = App {
            shot_id: 41,
            quit_id: 42,
            ..App::default()
        };

        for stage in [
            CaptureFailureStage::ReadCursor,
            CaptureFailureStage::CaptureImage,
        ] {
            install_test_capture(&mut app, RgbaImage::new(8, 6));
            app.capture_operation
                .as_mut()
                .unwrap()
                .seed_editing_text_for_test("private annotation", "secret text");

            app.begin_capture_attempt();
            assert_eq!(
                app.capture_operation.as_ref().map(CaptureOperation::phase),
                Some(CapturePhase::Preparing)
            );
            assert!(app.selection().is_none());

            let notice = app.take_capture_failure_notice(stage).unwrap();
            assert!(notice.contains(stage.code()));
            assert!(!notice.contains("secret text"));
            assert!(!notice.contains("private annotation"));
            assert!(app.capture_operation.is_none());
            assert!(app.selection().is_none());
            assert_eq!(app.shot_id, 41);
            assert_eq!(app.quit_id, 42);

            // 同一次尝试的重复失败不重复提示；下一轮 begin 后会再次提示。
            assert!(app.take_capture_failure_notice(stage).is_none());
        }
    }

    #[test]
    fn failed_session_is_cleared_once_and_hotkeys_survive() {
        let mut app = App {
            shot_id: 41,
            quit_id: 42,
            ..App::default()
        };
        install_test_capture(&mut app, RgbaImage::new(8, 6));
        app.capture_operation
            .as_mut()
            .unwrap()
            .seed_editing_text_for_test("测试", "ce");

        let notice = app.take_session_failure_notice(SessionFailure::new(
            SessionFailureStage::AcquireBuffer,
            "device lost",
        ));
        assert!(notice.is_some());
        assert!(app.capture_operation.is_none());
        assert!(app.selection().is_none());
        assert_eq!(app.shot_id, 41);
        assert_eq!(app.quit_id, 42);

        // 旧窗口排队事件再次到达时已没有活动资源，因此不会再次提示。
        assert!(
            app.take_session_failure_notice(SessionFailure::new(
                SessionFailureStage::Present,
                "stale event",
            ))
            .is_none()
        );

        // 下一次截图形成新会话后，新的失败仍然会产生一次通知。
        install_test_capture(&mut app, RgbaImage::new(2, 2));
        assert!(
            app.take_session_failure_notice(SessionFailure::new(
                SessionFailureStage::ResizeSurface,
                "new session",
            ))
            .is_some()
        );
    }

    #[test]
    fn session_failure_message_names_the_failed_stage() {
        let failure = SessionFailure::new(SessionFailureStage::Present, "device lost");
        assert_eq!(failure.to_string(), "提交绘制结果失败：device lost");
    }

    #[test]
    fn temp_png_allocation_is_unique_with_same_timestamp() {
        let dir = TestTempDir::new();
        let now = SystemTime::now();
        let img = RgbaImage::new(2, 2);
        let mut paths = HashSet::new();

        for _ in 0..32 {
            let path = write_unique_temp_png_in(&img, dir.path(), now).unwrap();
            assert_eq!(path.parent(), Some(dir.path()));
            assert!(is_managed_temp_png(&path));
            assert!(paths.insert(path.clone()));
            assert_eq!(&fs::read(path).unwrap()[..8], b"\x89PNG\r\n\x1a\n");
        }
    }

    #[test]
    fn cleanup_removes_only_expired_unprotected_managed_pngs() {
        let dir = TestTempDir::new();
        let now = SystemTime::now();
        let old = now - Duration::from_secs(13 * 60 * 60);
        let fresh = now - Duration::from_secs(60 * 60);
        let future = now + Duration::from_secs(60 * 60);

        let expired = dir.path().join("rshot-1-2-3.png");
        let recent = dir.path().join("rshot-1-2-4.png");
        let protected = dir.path().join("rshot-1-2-5.png");
        let future_dated = dir.path().join("rshot-1-2-6.png");
        let wrong_extension = dir.path().join("rshot-1-2-7.txt");
        let wrong_prefix = dir.path().join("other-1-2-8.png");
        let malformed = dir.path().join("rshot-not-managed.png");
        let named_directory = dir.path().join("rshot-1-2-9.png");

        create_file_with_mtime(&expired, old);
        create_file_with_mtime(&recent, fresh);
        create_file_with_mtime(&protected, old);
        create_file_with_mtime(&future_dated, future);
        create_file_with_mtime(&wrong_extension, old);
        create_file_with_mtime(&wrong_prefix, old);
        create_file_with_mtime(&malformed, old);
        fs::create_dir(&named_directory).unwrap();

        let removed = cleanup_expired_temp_pngs_in(
            dir.path(),
            now,
            TEMP_PNG_MAX_AGE,
            std::slice::from_ref(&protected),
        )
        .unwrap();

        assert_eq!(removed, 1);
        assert!(!expired.exists());
        for path in [
            recent,
            protected,
            future_dated,
            wrong_extension,
            wrong_prefix,
            malformed,
            named_directory,
        ] {
            assert!(path.exists(), "{} should be retained", path.display());
        }
    }

    #[test]
    fn temp_cleanup_runs_immediately_then_every_twelve_hours() {
        let start = Instant::now();
        let mut last_attempt = None;
        assert!(claim_temp_cleanup_slot(&mut last_attempt, start));
        assert_eq!(last_attempt, Some(start));
        assert!(!claim_temp_cleanup_slot(&mut last_attempt, start));
        assert!(!claim_temp_cleanup_slot(
            &mut last_attempt,
            start + TEMP_PNG_CLEANUP_INTERVAL - Duration::from_nanos(1)
        ));
        assert!(claim_temp_cleanup_slot(
            &mut last_attempt,
            start + TEMP_PNG_CLEANUP_INTERVAL
        ));
        assert_eq!(last_attempt, Some(start + TEMP_PNG_CLEANUP_INTERVAL));
    }
}

pub(super) fn entry() {
    if is_ocr_self_test_invocation() {
        if let Err(error) = run_ocr_self_test() {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }
    if is_ocr_worker_invocation() {
        if let Err(error) = run_ocr_worker() {
            eprintln!("{error}");
            std::process::exit(2);
        }
        return;
    }
    // release 版没控制台，启动出错会闷声退出；这里把错误弹窗告诉用户
    if let Err(error) = run() {
        show_message(&format!("rshot 启动失败：\n{error}"), true);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    // 最开头声明进程为 per-monitor-v2 DPI aware，赶在 EventLoop 和任何截图之前。
    // 否则高 DPI 屏上 winit 报逻辑尺寸、xcap 截物理尺寸，两者不一致会导致画面斜切。
    enable_per_monitor_dpi();
    let _winrt = WinRtApartment::initialize()?;

    let cfg: Config = confy::load("RShot", None)?;
    let shot_key: HotKey = cfg.hotkey.parse()?;
    let quit_key: HotKey = cfg.quit.parse()?;
    let about_message = build_about_message(&cfg.hotkey, &cfg.quit, cfg.diagnostics);

    // manager 要活到事件循环结束，否则热键会被注销，所以一直留在 main 作用域里
    let manager = GlobalHotKeyManager::new()?;
    manager.register(shot_key)?;
    manager.register(quit_key)?;

    println!(
        "配置文件: {}",
        confy::get_configuration_file_path("RShot", None)?.display()
    );

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    // 托盘图标：和 manager 一样要留在作用域活到循环结束，否则图标会消失
    let _tray = TrayIconBuilder::new()
        .with_tooltip("rshot")
        .with_icon(make_icon()?)
        .build()?;

    let app = App {
        shot_id: shot_key.id, // HotKey 是 Copy，register 后仍可取 id
        quit_id: quit_key.id,
        about_message,
        diagnostics_enabled: cfg.diagnostics,
        ..Default::default()
    };
    event_loop.run_app(app)?;

    drop(manager); // 显式让 manager 活到这里
    Ok(())
}
