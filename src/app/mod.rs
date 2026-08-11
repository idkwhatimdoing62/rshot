mod clipboard;
mod editor;
mod geometry;
mod handler;
mod ocr;
mod pinned;
mod render;
mod state;
mod windows_adapter;

use clipboard::*;
use editor::*;
use geometry::*;
use ocr::*;
use pinned::*;
use render::*;
use state::*;
use windows_adapter::*;

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use softbuffer::{Context, Surface};
use std::collections::HashMap;
use std::error::Error;
use std::num::NonZeroU32;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};
use tray_icon::{TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};
use xcap::Monitor;
use xcap::image::RgbaImage;

#[derive(Serialize, Deserialize)]
struct Config {
    hotkey: String,
    quit: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: "Alt+A".into(),
            quit: "Alt+D".into(),
        }
    }
}

const AUTHOR: &str = "idkwhatimdoing62";
const REPOSITORY_URL: &str = "https://github.com/idkwhatimdoing62/rshot";

fn build_about_message(hotkey: &str, quit: &str) -> String {
    format!(
        "rshot v{}\n\n作者：{}\n项目：{}\n\n当前快捷键\n截图：{}\n退出：{}\n\n截图只能通过全局热键触发。\n修改配置后请重启程序。",
        env!("CARGO_PKG_VERSION"),
        AUTHOR,
        REPOSITORY_URL,
        hotkey,
        quit,
    )
}

#[derive(Default)]
struct App {
    // 两个热键的 id，用来分辨收到的是哪一个
    shot_id: u32,
    quit_id: u32,
    about_message: String,

    // —— 以下是遮罩窗口的状态，只有正在框选时才有值 ——
    window: Option<Rc<dyn Window>>,
    surface: Option<Surface<Rc<dyn Window>, Rc<dyn Window>>>,
    img: Option<RgbaImage>, // 原始截图（裁剪用，保留 RGBA）
    cursor: (i32, i32),
    start: Option<(i32, i32)>,             // 拖动中的锚点
    cur: (i32, i32),                       // 鼠标当前点
    sel: Option<((i32, i32), (i32, i32))>, // 已定的选框（两对角点）

    // —— 自动锁定窗口用 ——
    windows: Vec<RectI>, // 开遮罩前拍下的所有窗口矩形（屏幕坐标，Z 序，顶层在前）
    origin: (i32, i32),  // 遮罩所在屏的左上角屏幕坐标，做窗口↔屏幕坐标换算
    dragged: bool,       // 本次按下后是否已构成拖拽（区分单击 vs 拖框）
    manual: bool,        // 已手动拖出选框、待右击确认。true 时停掉悬停锁定，别把框冲掉
    editor: EditorState,
    last_blink: Option<Instant>,
    modifiers: ModifiersState,
    pins: HashMap<WindowId, PinnedWindow>,
    last_temp_cleanup: Option<Instant>,
}

impl Deref for App {
    type Target = EditorState;

    fn deref(&self) -> &Self::Target {
        &self.editor
    }
}

impl DerefMut for App {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.editor
    }
}

impl App {
    /// 截图热键触发：截鼠标那块屏 + 弹全屏遮罩，进入框选
    fn open_overlay(&mut self, event_loop: &dyn ActiveEventLoop) {
        // 1. 鼠标坐标（进程已 DPI aware，拿的是物理像素）
        let Some(cursor) = cursor_position() else {
            return;
        };
        self.cursor = cursor;

        // 2. 截鼠标所在屏：转成显示用像素 + 留一份原图
        let Ok(monitor) = Monitor::from_point(cursor.0, cursor.1) else {
            return;
        };
        // 旧贴图在整个新会话期间保持隐藏，既不会进入截图，也不会盖住选择遮罩。
        self.set_pins_visible(false);
        let img = match monitor.capture_image() {
            Ok(img) => img,
            Err(_) => {
                self.set_pins_visible(true);
                return;
            }
        };
        self.img = Some(img);
        self.start = None; // 每次开都重置框选
        self.sel = None;
        self.editor.reset_for_capture();

        // 3. 找鼠标那块 winit 屏，建全屏无边框窗口钉上去
        let (cx, cy) = self.cursor;
        let target = event_loop.available_monitors().find(|m| {
            let (Some(pos), Some(mode)) = (m.position(), m.current_video_mode()) else {
                return false;
            };
            let size = mode.size();
            cx >= pos.x
                && cy >= pos.y
                && cx < pos.x + size.width as i32
                && cy < pos.y + size.height as i32
        });
        // 记下这块屏的左上角，做窗口坐标↔屏幕坐标换算
        self.origin = target
            .as_ref()
            .and_then(|m| m.position())
            .map(|pos| (pos.x, pos.y))
            .unwrap_or((0, 0));
        // 弹遮罩之前把所有可见窗口的矩形拍个快照（之后遮罩会盖住一切，就点不到底下窗口了）
        self.windows = visible_window_rects();

        let window: Rc<dyn Window> = match event_loop.create_window(
            WindowAttributes::default()
                .with_fullscreen(Some(winit::monitor::Fullscreen::Borderless(target))),
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
        self.window = Some(window);
        self.surface = Some(surface);
    }

    /// 确认截图：手动拖出的框裁框，否则截整屏。截完进剪贴板并收起遮罩。
    fn confirm(&mut self) {
        if self.img.is_some() {
            if let Some(w) = &self.window {
                w.set_visible(false); // 先藏，编码耗时挪到看不见后
            }
            if self.sel.is_none() && self.annotations.is_empty() {
                // 全屏且没有标注时直接使用原图，避免再克隆一份整屏 RGBA。
                if let Some(img) = self.img.as_ref() {
                    image_to_clipboard(img);
                }
            } else if let Some(img) = self.output_image() {
                image_to_clipboard(&img);
            }
        }
        self.close_overlay();
    }

    fn output_image(&self) -> Option<RgbaImage> {
        compose_output(self.img.as_ref()?, self.sel, &self.annotations)
    }

    /// 识别当前原始选区中的文字并写入文字剪贴板。标注层不会参与 OCR。
    fn copy_ocr_text(&mut self) {
        self.commit_text();
        let Some(img) = self.img.as_ref() else {
            return;
        };
        if let Some(w) = &self.window {
            w.set_visible(false);
        }

        let result = recognize_image_text(img, self.sel);
        match result {
            Ok(text) if text.is_empty() => {
                show_message("未识别到文字。\n请缩小选区，并确保文字足够清晰。", false);
                if let Some(w) = &self.window {
                    w.set_visible(true);
                    w.request_redraw();
                }
            }
            Ok(text) if text_to_clipboard(&text) => self.close_overlay(),
            Ok(_) => {
                show_message("文字已识别，但写入剪贴板失败，请重试。", true);
                if let Some(w) = &self.window {
                    w.set_visible(true);
                    w.request_redraw();
                }
            }
            Err(error) => {
                show_message(&format!("文字识别失败：\n{error}"), true);
                if let Some(w) = &self.window {
                    w.set_visible(true);
                    w.request_redraw();
                }
            }
        }
    }

    fn pin(&mut self) {
        if !has_pin_capacity(self.pins.len()) {
            if let Some(window) = &self.window {
                window.set_visible(false);
            }
            show_message(
                &format!(
                    "最多同时保留 {MAX_PINNED_WINDOWS} 张置顶贴图。\n请取消当前截图，关闭一张旧贴图后再试。"
                ),
                false,
            );
            if let Some(window) = &self.window {
                window.set_visible(true);
                window.request_redraw();
            }
            return;
        }
        let Some(window) = self.window.as_ref().cloned() else {
            return;
        };
        if self.surface.is_none() {
            self.handle_session_failure(SessionFailure::new(
                SessionFailureStage::AccessSurface,
                "活动截图窗口没有对应的绘图表面",
            ));
            return;
        }
        let out = if self.sel.is_none() && self.annotations.is_empty() {
            // 没有裁剪/标注时直接转移所有权，不再复制整屏。
            let Some(img) = self.img.take() else {
                return;
            };
            img
        } else {
            let Some(img) = self.output_image() else {
                return;
            };
            img
        };
        let pos = self
            .sel
            .map(normalized_rect)
            .map(|r| PhysicalPosition::new(self.origin.0 + r.0, self.origin.1 + r.1))
            .unwrap_or(PhysicalPosition::new(self.origin.0, self.origin.1));
        // 给右上角关闭按钮留出最小可点击区域，极小选区也不会丢失控制入口。
        let size = PhysicalSize::new(out.width().max(56), out.height().max(44));
        window.set_fullscreen(None);
        window.set_decorations(false);
        window.set_window_level(WindowLevel::AlwaysOnTop);
        let _ = window.request_surface_size(size.into());
        window.set_outer_position(pos.into());

        let Some(surface) = self.surface.take() else {
            self.handle_session_failure(SessionFailure::new(
                SessionFailureStage::AccessSurface,
                "转换贴图时绘图表面已经失效",
            ));
            return;
        };
        let Some(window) = self.window.take() else {
            self.surface = Some(surface);
            self.handle_session_failure(SessionFailure::new(
                SessionFailureStage::CreateWindow,
                "转换贴图时活动窗口已经失效",
            ));
            return;
        };
        let id = window.id();
        self.clear_capture_state();
        if let Some(replaced) = self
            .pins
            .insert(id, PinnedWindow::new(surface, window, out))
        {
            replaced.close();
        }
        self.set_pins_visible(true);
        if let Some(pin) = self.pins.get(&id) {
            pin.request_redraw();
        }
    }

    fn apply_toolbar_item(&mut self, item: ToolbarItem) {
        // 点工具栏先把未提交的文字输入落地，避免残留半截草稿
        self.commit_text();
        match item {
            ToolbarItem::Tool(tool) => {
                self.tool = tool;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            ToolbarItem::Color => {
                self.palette_open = !self.palette_open;
                self.palette_pressed = None;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            ToolbarItem::Action(action) => match action {
                ToolbarAction::Copy => self.confirm(),
                ToolbarAction::Ocr => self.copy_ocr_text(),
                ToolbarAction::Reselect => self.reselect(),
                ToolbarAction::Pin => self.pin(),
                ToolbarAction::Undo => {
                    self.annotations.pop();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                ToolbarAction::Close => self.close_overlay(),
            },
        }
    }

    fn close_palette(&mut self) {
        self.editor.close_palette();
    }

    /// 选中色板颜色：更新当前颜色；若正在输入文字，同步改掉草稿颜色（放框后再选色也能立即生效）。
    fn set_color(&mut self, index: usize) {
        self.editor.set_color(index);
    }

    /// 在编辑区按下左键：按当前工具起一条标注（拖动中实时更新，松手才定型）。
    fn start_shape(&mut self, p: (i32, i32)) {
        self.editor.start_shape(p);
    }

    /// 拖动中：更新最后一条标注的末端。
    fn update_draft(&mut self, p: (i32, i32)) {
        self.editor.update_draft(p);
    }

    /// 松手定型：单点画笔补成"点"，退化（起点=终点）的直线/矩形丢弃。
    fn commit_draft(&mut self) {
        self.editor.commit_draft();
    }

    /// 在编辑区用文字工具按下：先把旧的文字输入提交掉，再在点下处新建一条空的文字草稿。
    fn start_text(&mut self, p: (i32, i32)) {
        self.editor.start_text(p);
        self.last_blink = None;
        self.update_ime_area();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 提交文字输入：空内容则丢弃草稿。用于回车、点工具栏、切工具等时机。
    fn commit_text(&mut self) {
        if !self.editor.commit_text() {
            return;
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 取消文字输入：丢掉草稿。
    fn cancel_text(&mut self) {
        if !self.editor.cancel_text() {
            return;
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 把输入法候选窗定位到光标处（文字标注末尾）。
    #[allow(deprecated)]
    fn update_ime_area(&self) {
        if let Some(w) = &self.window {
            if let Some((x, y)) = self.caret_pos() {
                w.set_ime_cursor_area(
                    PhysicalPosition::new(x, y).into(),
                    PhysicalSize::new(2, TEXT_FONT_HEIGHT + 4).into(),
                );
            }
        }
    }

    /// 当前光标位置（文字末尾，含组合中拼音）的窗口内坐标。
    fn caret_pos(&self) -> Option<(i32, i32)> {
        let ann = self.annotations.last()?;
        if let Shape::Text(pos, text) = &ann.shape {
            let full = format!("{text}{}", self.ime_preedit);
            let (tw, _) = gdi_text_size(&full);
            Some((pos.0 + tw, pos.1))
        } else {
            None
        }
    }

    fn reselect(&mut self) {
        // 保留当前冻结画面，只清掉选区和标注，避免重新截屏造成内容变化。
        self.sel = None;
        self.start = None;
        self.dragged = false;
        self.manual = false;
        self.editor.reset_for_reselect();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn redraw_session(&mut self, window_id: WindowId) -> Result<(), SessionFailure> {
        let Some(window) = self.window.as_ref().cloned() else {
            // 窗口关闭后仍可能收到已经排队的 RedrawRequested；直接忽略。
            return Ok(());
        };
        if window.id() != window_id {
            // 旧窗口的排队事件不能影响随后打开的新截图会话。
            return Ok(());
        }
        let editor = &self.editor;
        let surface = self.surface.as_mut().ok_or_else(|| {
            SessionFailure::new(
                SessionFailureStage::AccessSurface,
                "活动窗口没有对应的绘图表面",
            )
        })?;
        let size = window.surface_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            // 最小化或窗口系统过渡期间可能短暂得到零尺寸，这不是会话故障。
            return Ok(());
        };
        surface
            .resize(w, h)
            .map_err(|error| SessionFailure::new(SessionFailureStage::ResizeSurface, error))?;
        let mut buffer = surface
            .buffer_mut()
            .map_err(|error| SessionFailure::new(SessionFailureStage::AcquireBuffer, error))?;

        if let Some(img) = &self.img {
            blit_rgba_image(&mut buffer[..], w.get(), h.get(), img);
        }
        if let Some((a, b)) = self.sel {
            shade_outside(&mut buffer[..], w.get(), h.get(), a, b);
            // 再盖 3 像素红框
            draw_rect(
                &mut buffer[..],
                w.get(),
                h.get(),
                a.0,
                a.1,
                b.0,
                b.1,
                0x00FF0000,
                3,
            );
        }
        for ann in &editor.annotations {
            draw_annotation_buffer(&mut buffer[..], w.get(), h.get(), ann);
        }
        if editor.mode == Mode::Editing {
            // 文字输入中的草稿：在文字上方画输入框 + 光标
            if editor.text_editing {
                if let Some(ann) = editor.annotations.last() {
                    draw_text_edit_box(
                        &mut buffer[..],
                        w.get(),
                        h.get(),
                        ann,
                        &editor.ime_preedit,
                        editor.cursor_visible,
                    );
                }
            }
            draw_toolbar(
                &mut buffer[..],
                w.get(),
                h.get(),
                self.sel,
                editor.tool,
                editor.color,
                editor.toolbar_hover,
                editor.palette_open,
            );
            if editor.palette_open {
                draw_palette_popup(
                    &mut buffer[..],
                    w.get(),
                    h.get(),
                    self.sel,
                    editor.color,
                    editor.palette_hover,
                );
            }
        } else {
            draw_select_badge(&mut buffer[..], w.get(), h.get());
        }
        buffer
            .present()
            .map_err(|error| SessionFailure::new(SessionFailureStage::Present, error))
    }

    /// 清理失败会话并返回是否真的存在活动资源；用于屏蔽旧事件造成的重复报错。
    fn recover_failed_session(&mut self) -> bool {
        let was_active = self.window.is_some() || self.surface.is_some() || self.img.is_some();
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

    fn clear_capture_state(&mut self) {
        self.img = None;
        self.start = None;
        self.sel = None;
        self.windows = Vec::new();
        self.dragged = false;
        self.manual = false;
        self.editor.reset_for_reselect();
    }

    /// 关掉活动截图窗口，回后台待命；已有贴图恢复显示且不被销毁。
    fn close_overlay(&mut self) {
        // Surface 可能持有窗口句柄，先释放它，再释放窗口。
        if let Some(window) = self.window.as_ref() {
            window.set_visible(false);
        }
        drop(self.surface.take());
        drop(self.window.take());
        self.clear_capture_state();
        self.set_pins_visible(true);
    }

    /// 光标当前所在的窗口矩形（转成窗口内坐标）。没命中返回 None
    fn window_under_cursor(&self) -> Option<((i32, i32), (i32, i32))> {
        let sx = self.cur.0 + self.origin.0; // 窗口坐标 → 屏幕坐标
        let sy = self.cur.1 + self.origin.1;
        for r in &self.windows {
            if sx >= r.left && sx < r.right && sy >= r.top && sy < r.bottom {
                // 顶层在前，第一个命中就是最上面那个窗口。
                // 四边各内缩 1px：DWM 边界会带上窗口自身那圈 1px 边框，去掉免得截到缝
                return Some((
                    (r.left - self.origin.0 + 1, r.top - self.origin.1 + 1),
                    (r.right - self.origin.0 - 1, r.bottom - self.origin.1 - 1),
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Annotation, App, MAX_PINNED_WINDOWS, Mode, PALETTE, SessionFailure, SessionFailureStage,
        Shape, TEMP_PNG_CLEANUP_INTERVAL, TEMP_PNG_MAX_AGE, TEXT_FONT_HEIGHT, TOOLBAR_SLOT_COLOR,
        TOOLBAR_SLOT_COUNT, Tool, ToolbarAction, ToolbarItem, blit_rgba_image, build_about_message,
        build_dib, claim_temp_cleanup_slot, cleanup_expired_temp_pngs_in, color_u32, crop_image,
        dragged_window_position, draw_annotation_image, draw_line_image, draw_rect_image,
        gdi_text_size, has_pin_capacity, is_managed_temp_png, normalized_rect, ocr_region,
        palette_hit, palette_popup_rect, palette_swatch_rect, prepare_ocr_rgba, toolbar_hit,
        toolbar_item, toolbar_item_rect, toolbar_item_slot, toolbar_origin, toolbar_size,
        unicode_text_bytes, write_unique_temp_png_in,
    };
    use std::collections::HashSet;
    use std::fs::{self, FileTimes, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use xcap::image::RgbaImage;

    static TEST_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn about_message_shows_author_and_loaded_hotkeys() {
        let message = build_about_message("Ctrl+Shift+S", "Ctrl+Shift+Q");

        assert!(message.contains("idkwhatimdoing62"));
        assert!(message.contains("Ctrl+Shift+S"));
        assert!(message.contains("Ctrl+Shift+Q"));
        assert!(message.contains("截图只能通过全局热键触发"));
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
    fn crop_accepts_reverse_drag_and_clamps_to_image() {
        let img = RgbaImage::new(10, 8);
        let cropped = crop_image(&img, (9, 7), (-3, 2)).unwrap();
        assert_eq!(cropped.dimensions(), (9, 5));
        assert_eq!(normalized_rect(((9, 7), (-3, 2))), (-3, 2, 9, 7));
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
    fn reselect_keeps_frozen_image_but_clears_edit_state() {
        let mut app = App::default();
        app.img = Some(RgbaImage::new(12, 9));
        app.mode = Mode::Editing;
        app.sel = Some(((1, 2), (8, 7)));
        app.annotations.push(Annotation {
            shape: Shape::Pen(vec![(2, 3), (4, 5)]),
            color: [255, 0, 0, 255],
        });
        app.tool = Tool::Line;
        app.reselect();
        assert_eq!(app.mode, Mode::Selecting);
        assert!(app.img.is_some());
        assert!(app.sel.is_none());
        assert!(app.annotations.is_empty());
        assert_eq!(app.tool, Tool::Line);
    }

    #[test]
    fn commit_draft_drops_degenerate_line() {
        let mut app = App::default();
        app.annotations.push(Annotation {
            shape: Shape::Line((5, 5), (5, 5)),
            color: [255, 0, 0, 255],
        });
        app.commit_draft();
        assert!(app.annotations.is_empty());
    }

    #[test]
    fn commit_draft_turns_single_pen_point_into_dot() {
        let mut app = App::default();
        app.annotations.push(Annotation {
            shape: Shape::Pen(vec![(3, 3)]),
            color: [255, 0, 0, 255],
        });
        app.commit_draft();
        match &app.annotations[0].shape {
            Shape::Pen(p) => assert_eq!(p.len(), 2),
            _ => panic!("expected pen"),
        }
    }

    #[test]
    fn start_shape_uses_current_tool_and_color() {
        let mut app = App::default();
        app.tool = Tool::Rect;
        app.color = PALETTE[4];
        app.start_shape((7, 9));
        assert_eq!(app.annotations.len(), 1);
        assert_eq!(app.annotations[0].color, PALETTE[4]);
        assert_eq!(app.annotations[0].shape, Shape::Rect((7, 9), (7, 9)));
    }

    #[test]
    fn update_draft_moves_line_endpoint() {
        let mut app = App::default();
        app.tool = Tool::Line;
        app.start_shape((1, 1));
        app.update_draft((6, 6));
        assert_eq!(app.annotations[0].shape, Shape::Line((1, 1), (6, 6)));
    }

    #[test]
    fn start_text_creates_draft_and_commits_empty_away() {
        let mut app = App::default();
        app.mode = Mode::Editing;
        app.start_text((5, 5));
        assert!(app.text_editing);
        assert_eq!(app.annotations.len(), 1);
        assert!(matches!(app.annotations[0].shape, Shape::Text((5, 5), ref s) if s.is_empty()));
        app.commit_text();
        assert!(!app.text_editing);
        assert!(app.annotations.is_empty());
    }

    #[test]
    fn cancel_text_drops_draft() {
        let mut app = App::default();
        app.mode = Mode::Editing;
        app.start_text((5, 5));
        app.cancel_text();
        assert!(!app.text_editing);
        assert!(app.annotations.is_empty());
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
    fn commit_and_cancel_clear_ime_preedit() {
        let mut app = App::default();
        app.mode = Mode::Editing;
        app.start_text((5, 5));
        app.ime_preedit = String::from("ni");
        app.commit_text();
        assert!(app.ime_preedit.is_empty());
        app.start_text((6, 6));
        app.ime_preedit = String::from("hao");
        app.cancel_text();
        assert!(app.ime_preedit.is_empty());
    }

    #[test]
    fn set_color_updates_editing_text_draft() {
        let mut app = App::default();
        app.mode = Mode::Editing;
        app.color = PALETTE[0];
        app.start_text((5, 5)); // 红色草稿
        assert_eq!(app.annotations[0].color, PALETTE[0]);
        app.set_color(4); // 输入中换蓝
        assert_eq!(app.color, PALETTE[4]);
        assert_eq!(app.annotations[0].color, PALETTE[4]);
    }

    #[test]
    fn failed_session_is_cleared_once_and_hotkeys_survive() {
        let mut app = App {
            shot_id: 41,
            quit_id: 42,
            img: Some(RgbaImage::new(8, 6)),
            start: Some((1, 1)),
            sel: Some(((1, 1), (5, 4))),
            dragged: true,
            manual: true,
            ..App::default()
        };
        app.mode = Mode::Editing;
        app.drawing = true;
        app.palette_open = true;
        app.text_editing = true;
        app.ime_preedit = String::from("ce");
        app.annotations.push(Annotation {
            shape: Shape::Text((1, 1), String::from("测")),
            color: PALETTE[0],
        });

        let notice = app.take_session_failure_notice(SessionFailure::new(
            SessionFailureStage::AcquireBuffer,
            "device lost",
        ));
        assert!(notice.is_some());
        assert!(app.window.is_none());
        assert!(app.surface.is_none());
        assert!(app.img.is_none());
        assert!(app.start.is_none());
        assert!(app.sel.is_none());
        assert!(app.annotations.is_empty());
        assert!(!app.dragged);
        assert!(!app.manual);
        assert!(!app.drawing);
        assert!(!app.palette_open);
        assert!(!app.text_editing);
        assert!(app.ime_preedit.is_empty());
        assert_eq!(app.mode, Mode::Selecting);
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
        app.img = Some(RgbaImage::new(2, 2));
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
    let about_message = build_about_message(&cfg.hotkey, &cfg.quit);

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
        ..Default::default()
    };
    event_loop.run_app(app)?;

    drop(manager); // 显式让 manager 活到这里
    Ok(())
}
