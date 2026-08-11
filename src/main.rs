// release 构建切到 windows 子系统 = 双击不弹黑色控制台窗口。
// debug（cargo run）保留控制台，方便看 println!/panic。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use softbuffer::{Context, Surface};
use std::borrow::Cow;
use std::error::Error;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{
    COLORREF, GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, POINT, RECT,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_CALCRECT, DT_LEFT, DT_NOPREFIX, DT_TOP, DeleteDC, DeleteObject, DrawTextW,
    FF_DONTCARE, FW_NORMAL, GetDIBits, HGDIOBJ, OPAQUE, OUT_DEFAULT_PRECIS, SelectObject,
    SetBkColor, SetBkMode, SetTextColor,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Shell::DROPFILES;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindowRect, IsIconic, IsWindowVisible, MB_ICONERROR,
    MB_ICONINFORMATION, MessageBoxW,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};
use xcap::Monitor;
use xcap::image::{RgbaImage, imageops};

/// 主 UI 线程使用单线程 WinRT apartment；OCR 生命周期覆盖整个事件循环。
struct WinRtApartment;

impl WinRtApartment {
    fn initialize() -> windows::core::Result<Self> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        Ok(Self)
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

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

#[derive(Default, Debug, PartialEq)]
enum Mode {
    #[default]
    Selecting,
    Editing,
    Pinned,
}

/// 当前选中的标注工具（编辑模式下左键拖拽用哪个图元）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Pen,
    Line,
    Rect,
    Text,
}

impl Default for Tool {
    fn default() -> Self {
        Tool::Pen
    }
}

/// 一条标注的形状：自由画笔（一串点）/ 直线（两点）/ 矩形（两对角点）/ 文字（左上角锚点 + 内容）。
#[derive(Clone, Debug, PartialEq)]
enum Shape {
    Pen(Vec<(i32, i32)>),
    Line((i32, i32), (i32, i32)),
    Rect((i32, i32), (i32, i32)),
    Text((i32, i32), String),
}

/// 一条标注 = 形状 + 颜色（RGBA，输出图直接用；显示缓冲按 0RGB 转换）。
#[derive(Clone, Debug)]
struct Annotation {
    shape: Shape,
    color: [u8; 4],
}

/// 预设调色板：PEN 默认红放在第一位。
const PALETTE: [[u8; 4]; 8] = [
    [255, 45, 45, 255],   // 红
    [245, 102, 0, 255],   // 橙
    [255, 200, 0, 255],   // 黄
    [0, 166, 90, 255],    // 绿
    [59, 120, 200, 255],  // 蓝
    [107, 90, 168, 255],  // 紫
    [255, 255, 255, 255], // 白
    [0, 0, 0, 255],       // 黑
];

#[derive(Default)]
struct App {
    // 两个热键的 id，用来分辨收到的是哪一个
    shot_id: u32,
    quit_id: u32,

    // —— 以下是遮罩窗口的状态，只有正在框选时才有值 ——
    window: Option<Rc<dyn Window>>,
    surface: Option<Surface<Rc<dyn Window>, Rc<dyn Window>>>,
    img: Option<RgbaImage>, // 原始截图（裁剪用，保留 RGBA）
    cursor: (i32, i32),
    start: Option<(i32, i32)>,             // 拖动中的锚点
    cur: (i32, i32),                       // 鼠标当前点
    sel: Option<((i32, i32), (i32, i32))>, // 已定的选框（两对角点）

    // —— 自动锁定窗口用 ——
    windows: Vec<RECT>, // 开遮罩前拍下的所有窗口矩形（屏幕坐标，Z 序，顶层在前）
    origin: (i32, i32), // 遮罩所在屏的左上角屏幕坐标，做窗口↔屏幕坐标换算
    dragged: bool,      // 本次按下后是否已构成拖拽（区分单击 vs 拖框）
    manual: bool,       // 已手动拖出选框、待右击确认。true 时停掉悬停锁定，别把框冲掉
    mode: Mode,
    annotations: Vec<Annotation>,
    tool: Tool,
    color: [u8; 4],
    drawing: bool,
    toolbar_hover: Option<usize>,
    toolbar_pressed: Option<usize>,
    palette_open: bool,
    palette_hover: Option<usize>,
    palette_pressed: Option<usize>,
    text_editing: bool,   // 文字工具正在输入中（annotations 末尾那条 Text 是草稿）
    ime_preedit: String,  // 输入法组合中的拼音串（非空 = 组合中）
    cursor_visible: bool, // 文字输入光标闪烁状态
    last_blink: Option<Instant>,
    modifiers: ModifiersState,
    pin_drag: Option<((i32, i32), (i32, i32))>,
}

impl App {
    /// 截图热键触发：截鼠标那块屏 + 弹全屏遮罩，进入框选
    fn open_overlay(&mut self, event_loop: &dyn ActiveEventLoop) {
        // 1. 鼠标坐标（进程已 DPI aware，拿的是物理像素）
        let mut p = POINT::default();
        unsafe {
            if GetCursorPos(&mut p).is_err() {
                return;
            }
        }
        self.cursor = (p.x, p.y);

        // 2. 截鼠标所在屏：转成显示用像素 + 留一份原图
        let Ok(monitor) = Monitor::from_point(p.x, p.y) else {
            return;
        };
        let Ok(img) = monitor.capture_image() else {
            return;
        };
        self.img = Some(img);
        self.start = None; // 每次开都重置框选
        self.sel = None;
        self.mode = Mode::Selecting;
        self.annotations.clear();
        self.tool = Tool::Pen;
        self.color = PALETTE[0];
        self.drawing = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        self.text_editing = false;
        self.ime_preedit.clear();
        self.close_palette();
        self.pin_drag = None;

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
        let mut wins: Vec<RECT> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(enum_windows_cb),
                LPARAM(&mut wins as *mut Vec<RECT> as isize),
            );
        }
        self.windows = wins;

        let window: Rc<dyn Window> = Rc::from(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_fullscreen(Some(winit::monitor::Fullscreen::Borderless(target))),
                )
                .unwrap(),
        );
        let context = Context::new(window.clone()).unwrap();
        let surface = Surface::new(&context, window.clone()).unwrap();
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
        let img = self.img.as_ref()?;
        let mut out = match self.sel {
            Some((a, b)) => crop_image(img, a, b)?,
            None => img.clone(),
        };
        let offset = self
            .sel
            .map(normalized_rect)
            .map(|r| (r.0, r.1))
            .unwrap_or((0, 0));
        for ann in &self.annotations {
            draw_annotation_image(&mut out, ann, offset);
        }
        Some(out)
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
        let Some(window) = self.window.as_ref().cloned() else {
            return;
        };
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
        self.img = Some(out);
        self.sel = None;
        self.annotations.clear();
        self.drawing = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        self.text_editing = false;
        self.ime_preedit.clear();
        self.close_palette();
        self.mode = Mode::Pinned;
        self.pin_drag = None;
        window.set_fullscreen(None);
        window.set_decorations(false);
        window.set_window_level(WindowLevel::AlwaysOnTop);
        let _ = window.request_surface_size(size.into());
        window.set_outer_position(pos.into());
        window.request_redraw();
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
        self.palette_open = false;
        self.palette_hover = None;
        self.palette_pressed = None;
    }

    /// 选中色板颜色：更新当前颜色；若正在输入文字，同步改掉草稿颜色（放框后再选色也能立即生效）。
    fn set_color(&mut self, index: usize) {
        self.color = PALETTE[index];
        if self.text_editing {
            if let Some(last) = self.annotations.last_mut() {
                if matches!(last.shape, Shape::Text(..)) {
                    last.color = self.color;
                }
            }
        }
    }

    /// 在编辑区按下左键：按当前工具起一条标注（拖动中实时更新，松手才定型）。
    fn start_shape(&mut self, p: (i32, i32)) {
        let shape = match self.tool {
            Tool::Pen => Shape::Pen(vec![p]),
            Tool::Line => Shape::Line(p, p),
            Tool::Rect => Shape::Rect(p, p),
            Tool::Text => return, // 文字在按下时直接进入输入态，不走草稿流程
        };
        self.annotations.push(Annotation {
            shape,
            color: self.color,
        });
    }

    /// 拖动中：更新最后一条标注的末端。
    fn update_draft(&mut self, p: (i32, i32)) {
        let Some(ann) = self.annotations.last_mut() else {
            return;
        };
        match &mut ann.shape {
            Shape::Pen(points) => points.push(p),
            Shape::Line(_, b) => *b = p,
            Shape::Rect(_, b) => *b = p,
            Shape::Text(..) => {}
        }
    }

    /// 松手定型：单点画笔补成"点"，退化（起点=终点）的直线/矩形丢弃。
    fn commit_draft(&mut self) {
        let Some(ann) = self.annotations.last() else {
            return;
        };
        let (drop, dot) = match &ann.shape {
            Shape::Pen(points) => (false, points.len() == 1),
            Shape::Line(a, b) | Shape::Rect(a, b) => (*a == *b, false),
            Shape::Text(..) => (false, false),
        };
        if drop {
            self.annotations.pop();
        } else if dot {
            if let Some(Shape::Pen(points)) = self.annotations.last_mut().map(|a| &mut a.shape) {
                points.push(points[0]);
            }
        }
    }

    /// 在编辑区用文字工具按下：先把旧的文字输入提交掉，再在点下处新建一条空的文字草稿。
    fn start_text(&mut self, p: (i32, i32)) {
        self.commit_text();
        self.annotations.push(Annotation {
            shape: Shape::Text(p, String::new()),
            color: self.color,
        });
        self.text_editing = true;
        self.ime_preedit.clear();
        self.cursor_visible = true;
        self.last_blink = None;
        self.update_ime_area();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 提交文字输入：空内容则丢弃草稿。用于回车、点工具栏、切工具等时机。
    fn commit_text(&mut self) {
        if !self.text_editing {
            return;
        }
        self.text_editing = false;
        self.ime_preedit.clear();
        if let Some(last) = self.annotations.last() {
            if let Shape::Text(_, text) = &last.shape {
                if text.is_empty() {
                    self.annotations.pop();
                }
            }
        }
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 取消文字输入：丢掉草稿。
    fn cancel_text(&mut self) {
        if !self.text_editing {
            return;
        }
        self.text_editing = false;
        self.ime_preedit.clear();
        if let Some(last) = self.annotations.last() {
            if matches!(last.shape, Shape::Text(..)) {
                self.annotations.pop();
            }
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
        self.mode = Mode::Selecting;
        self.sel = None;
        self.start = None;
        self.dragged = false;
        self.manual = false;
        self.annotations.clear();
        self.drawing = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        self.text_editing = false;
        self.ime_preedit.clear();
        self.close_palette();
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 关掉遮罩窗口，回后台待命（不退程序）。丢掉所有 Rc，窗口即被销毁
    fn close_overlay(&mut self) {
        self.window = None;
        self.surface = None;
        self.img = None;
        self.start = None;
        self.sel = None;
        self.windows = Vec::new();
        self.dragged = false;
        self.manual = false;
        self.mode = Mode::Selecting;
        self.annotations.clear();
        self.drawing = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        self.text_editing = false;
        self.ime_preedit.clear();
        self.close_palette();
        self.pin_drag = None;
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

impl ApplicationHandler for App {
    // 本程序不在启动时建窗口，遮罩是热键触发后临时建的，这里留空
    fn can_create_surfaces(&mut self, _event_loop: &dyn ActiveEventLoop) {}

    // 每轮空闲：轮询 global-hotkey 的事件通道
    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if ev.id == self.quit_id {
                event_loop.exit(); // 退出整个程序
            } else if ev.id == self.shot_id && self.window.is_none() {
                // 没在框选时才响应，避免叠窗
                self.open_overlay(event_loop);
            }
        }
        // 托盘图标：双击 = 截图（跟收热键一个套路，多轮询一个通道）
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = ev {
                if self.window.is_none() {
                    self.open_overlay(event_loop);
                }
            }
        }
        // 文字输入光标闪烁：每 ~530ms 翻转一次可见性
        if self.text_editing && self.mode == Mode::Editing {
            let now = Instant::now();
            match self.last_blink {
                Some(last) if now.duration_since(last) >= Duration::from_millis(530) => {
                    self.cursor_visible = !self.cursor_visible;
                    self.last_blink = Some(now);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                None => {
                    self.last_blink = Some(now);
                    self.cursor_visible = true;
                }
                _ => {}
            }
        } else {
            self.last_blink = None;
            self.cursor_visible = true;
        }
        // ponytail: 120ms 轮询一次热键。想零延迟得用 EventLoopProxy 唤醒，暂不需要
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(120),
        ));
    }

    fn window_event(
        &mut self,
        _event_loop: &dyn ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => self.close_overlay(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    // 文字输入中：字符进缓冲区，退格删字，回车提交，Esc 取消
                    if self.text_editing && self.mode == Mode::Editing {
                        // 输入法组合中：按键交给 IME（拼音会走 Preedit/Commit），别自己处理，避免重复进缓冲
                        if !self.ime_preedit.is_empty() {
                            return;
                        }
                        if event.physical_key == PhysicalKey::Code(KeyCode::Backspace) {
                            if let Some(last) = self.annotations.last_mut() {
                                if let Shape::Text(_, text) = &mut last.shape {
                                    text.pop();
                                }
                            }
                            self.update_ime_area();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        if event.physical_key == PhysicalKey::Code(KeyCode::Enter) {
                            self.commit_text();
                            return;
                        }
                        if let Key::Named(NamedKey::Escape) = event.logical_key {
                            self.cancel_text();
                            return;
                        }
                        if let Some(text) = event.text {
                            if text.chars().all(|c| !c.is_control()) {
                                if let Some(last) = self.annotations.last_mut() {
                                    if let Shape::Text(_, buf) = &mut last.shape {
                                        buf.push_str(text.as_str());
                                    }
                                }
                                self.update_ime_area();
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                            }
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyP)
                        && self.mode != Mode::Pinned
                    {
                        self.pin();
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyZ)
                        && self.modifiers.control_key()
                        && self.mode == Mode::Editing
                    {
                        self.annotations.pop();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyB)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Pen;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyN)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Line;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyM)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Rect;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyT)
                        && self.mode == Mode::Editing
                    {
                        self.tool = Tool::Text;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyR)
                        && self.mode == Mode::Editing
                    {
                        self.reselect();
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyO)
                        && self.mode == Mode::Editing
                    {
                        self.copy_ocr_text();
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyC)
                        && self.mode != Mode::Pinned
                    {
                        self.confirm();
                        return;
                    }
                    if let Key::Named(NamedKey::Escape) = event.logical_key {
                        if self.palette_open {
                            self.close_palette();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        self.close_overlay();
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                // 输入法组合：只处理正在编辑文字时
                if self.text_editing && self.mode == Mode::Editing {
                    match ime {
                        Ime::Preedit(text, _cursor) => {
                            // 首次进入组合：若键盘事件已把同样的拼音塞进草稿尾部，先去掉避免重复
                            if self.ime_preedit.is_empty() && !text.is_empty() {
                                if let Some(last) = self.annotations.last_mut() {
                                    if let Shape::Text(_, buf) = &mut last.shape {
                                        let n = text.chars().count();
                                        let tail: String = buf
                                            .chars()
                                            .rev()
                                            .take(n)
                                            .collect::<Vec<_>>()
                                            .into_iter()
                                            .rev()
                                            .collect();
                                        if tail == text {
                                            for _ in 0..n {
                                                buf.pop();
                                            }
                                        }
                                    }
                                }
                            }
                            self.ime_preedit = text;
                            self.update_ime_area();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                        Ime::Commit(text) => {
                            self.ime_preedit.clear();
                            if let Some(last) = self.annotations.last_mut() {
                                if let Shape::Text(_, buf) = &mut last.shape {
                                    buf.push_str(&text);
                                }
                            }
                            self.update_ime_area();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                        Ime::Enabled => {
                            self.update_ime_area();
                        }
                        Ime::Disabled | Ime::DeleteSurrounding { .. } => {
                            self.ime_preedit.clear();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::PointerMoved { position, .. } => {
                self.cur = (position.x as i32, position.y as i32);
                if self.mode == Mode::Editing {
                    let hover = self
                        .window
                        .as_ref()
                        .map(|w| w.surface_size())
                        .and_then(|size| {
                            toolbar_hit(self.cur, size.width as i32, size.height as i32, self.sel)
                        });
                    let hover_index = hover.map(toolbar_item_slot);
                    let hover_changed = hover_index != self.toolbar_hover;
                    self.toolbar_hover = hover_index;
                    let palette_hover = if self.palette_open {
                        self.window.as_ref().and_then(|w| {
                            let size = w.surface_size();
                            palette_hit(self.cur, size.width as i32, size.height as i32, self.sel)
                        })
                    } else {
                        None
                    };
                    let palette_changed = palette_hover != self.palette_hover;
                    self.palette_hover = palette_hover;
                    if self.drawing {
                        self.update_draft(self.cur);
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    } else if hover_changed || palette_changed {
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                    return;
                }
                if self.mode == Mode::Pinned {
                    if let Some((cursor_start, window_start)) = self.pin_drag {
                        let mut cursor = POINT::default();
                        if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
                            if let Some(w) = &self.window {
                                let x = window_start.0 + cursor.x - cursor_start.0;
                                let y = window_start.1 + cursor.y - cursor_start.1;
                                w.set_outer_position(PhysicalPosition::new(x, y).into());
                            }
                        }
                    }
                    return;
                }
                let before = self.sel;
                match self.start {
                    // 按住中：移动超过 4 像素才算拖框，否则保持（留给单击截窗）
                    Some(anchor) => {
                        if (self.cur.0 - anchor.0).abs() > 4 || (self.cur.1 - anchor.1).abs() > 4 {
                            self.dragged = true;
                            self.sel = Some((anchor, self.cur));
                        }
                    }
                    // 没按住：悬停锁定光标下的窗口。但已手动拖过框就别再冲掉它
                    None => {
                        if !self.manual {
                            self.sel = self.window_under_cursor();
                        }
                    }
                }
                // 选框变了才重画，省得原地不动也刷屏
                if self.sel != before {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::PointerButton { state, button, .. } => {
                let mb = button.mouse_button();
                if self.mode == Mode::Editing {
                    let toolbar_item =
                        self.window
                            .as_ref()
                            .map(|w| w.surface_size())
                            .and_then(|size| {
                                toolbar_hit(
                                    self.cur,
                                    size.width as i32,
                                    size.height as i32,
                                    self.sel,
                                )
                            });
                    let palette_swatch = if self.palette_open {
                        self.window.as_ref().and_then(|w| {
                            let size = w.surface_size();
                            palette_hit(self.cur, size.width as i32, size.height as i32, self.sel)
                        })
                    } else {
                        None
                    };
                    if mb == Some(MouseButton::Left) {
                        match state {
                            ElementState::Pressed => {
                                // 优先级：色板色块 > 工具栏按钮 > 画布
                                if let Some(i) = palette_swatch {
                                    self.palette_pressed = Some(i);
                                    return;
                                }
                                if toolbar_item.is_some() {
                                    self.toolbar_pressed = toolbar_item.map(toolbar_item_slot);
                                    return;
                                }
                                // 色板开着时点画布只关菜单，不画标注
                                if self.palette_open {
                                    self.close_palette();
                                    return;
                                }
                                if point_in_selection(self.cur, self.sel) {
                                    if self.tool == Tool::Text {
                                        self.start_text(self.cur);
                                    } else {
                                        self.drawing = true;
                                        self.start_shape(self.cur);
                                    }
                                }
                            }
                            ElementState::Released => {
                                if let Some(pressed) = self.palette_pressed.take() {
                                    if palette_swatch == Some(pressed) {
                                        self.set_color(pressed);
                                        self.close_palette();
                                        if let Some(w) = &self.window {
                                            w.request_redraw();
                                        }
                                    }
                                    return;
                                }
                                if toolbar_item.is_some() {
                                    let pressed = self.toolbar_pressed.take();
                                    let slot = toolbar_item.map(toolbar_item_slot);
                                    if pressed.is_some() && pressed == slot {
                                        self.apply_toolbar_item(toolbar_item.unwrap());
                                    }
                                    return;
                                }
                                if self.drawing {
                                    self.commit_draft();
                                    if let Some(w) = &self.window {
                                        w.request_redraw();
                                    }
                                    self.drawing = false;
                                }
                            }
                        }
                        return;
                    }
                    if state == ElementState::Released {
                        self.toolbar_pressed = None;
                        self.palette_pressed = None;
                    }
                }
                if self.mode == Mode::Pinned {
                    if mb == Some(MouseButton::Right) && state == ElementState::Released {
                        self.close_overlay();
                    } else if mb == Some(MouseButton::Left) {
                        let close_hit = self
                            .window
                            .as_ref()
                            .map(|w| {
                                let size = w.surface_size();
                                let r = pin_close_rect(size.width as i32, size.height as i32);
                                self.cur.0 >= r.0
                                    && self.cur.0 < r.2
                                    && self.cur.1 >= r.1
                                    && self.cur.1 < r.3
                            })
                            .unwrap_or(false);
                        match state {
                            ElementState::Pressed => {
                                if close_hit {
                                    self.close_overlay();
                                    return;
                                }
                                let mut cursor = POINT::default();
                                if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
                                    if let Some(pos) =
                                        self.window.as_ref().and_then(|w| w.outer_position().ok())
                                    {
                                        self.pin_drag =
                                            Some(((cursor.x, cursor.y), (pos.x, pos.y)));
                                    }
                                }
                            }
                            ElementState::Released => {
                                self.pin_drag = None;
                            }
                        }
                    }
                    return;
                }
                // 右键抬起 = 确认（有手动框裁框，否则全屏）。
                // 必须等抬起：若按下就关遮罩，抬起那半下会漏给下面窗口，触发系统右键菜单
                if mb == Some(MouseButton::Right) && state == ElementState::Released {
                    self.confirm();
                } else if mb == Some(MouseButton::Left) {
                    if self.mode == Mode::Editing {
                        match state {
                            ElementState::Pressed => {
                                if point_in_selection(self.cur, self.sel) {
                                    self.drawing = true;
                                    self.start_shape(self.cur);
                                }
                            }
                            ElementState::Released => {
                                if self.drawing {
                                    self.commit_draft();
                                    if let Some(w) = &self.window {
                                        w.request_redraw();
                                    }
                                }
                                self.drawing = false;
                            }
                        }
                        return;
                    }
                    match state {
                        ElementState::Pressed => {
                            // 按下先记锚点；sel 保持（可能是悬停锁定的窗口），供单击截取
                            self.start = Some(self.cur);
                            self.dragged = false;
                            self.manual = false; // 重新开框，解除上次的手动锁定
                        }
                        ElementState::Released => {
                            let was_drag = self.dragged;
                            self.start = None;
                            self.dragged = false;
                            if !was_drag {
                                // 单击锁定窗口后进入编辑态，避免误触直接结束。
                                if self.sel.is_some() {
                                    self.manual = true;
                                    self.mode = Mode::Editing;
                                    self.toolbar_hover = None;
                                    self.toolbar_pressed = None;
                                }
                            }
                            // 拖框后进入编辑态：工具栏选工具/颜色，左键画标注。
                            else {
                                self.manual = true;
                                self.mode = Mode::Editing;
                                self.toolbar_hover = None;
                                self.toolbar_pressed = None;
                            }
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(window) = self.window.as_ref() else {
                    return;
                };
                let surface = self.surface.as_mut().unwrap();
                let size = window.surface_size();
                let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                else {
                    return;
                };
                surface.resize(w, h).unwrap();
                let mut buffer = surface.buffer_mut().unwrap();
                // 逐行铺冻屏：按“截图宽度”对齐每一行。
                // 若用一维 copy，一旦窗口宽 ≠ 截图宽，整幅会斜掉（右边错位）
                if let Some(img) = &self.img {
                    let iw = img.width() as usize;
                    let ih = img.height() as usize;
                    let sw = w.get() as usize;
                    let sh = h.get() as usize;
                    if iw != sw || ih != sh {
                        buffer.fill(0); // 尺寸不齐时先铺黑，右/下留边不显示脏数据
                    }
                    let copy_w = iw.min(sw);
                    let raw = img.as_raw();
                    for y in 0..ih.min(sh) {
                        let src = &raw[y * iw * 4..(y * iw + copy_w) * 4];
                        let dst = &mut buffer[y * sw..y * sw + copy_w];
                        for x in 0..copy_w {
                            let i = x * 4;
                            dst[x] = (src[i] as u32) << 16
                                | (src[i + 1] as u32) << 8
                                | src[i + 2] as u32;
                        }
                    }
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
                for ann in &self.annotations {
                    draw_annotation_buffer(&mut buffer[..], w.get(), h.get(), ann);
                }
                if self.mode == Mode::Editing {
                    // 文字输入中的草稿：在文字上方画输入框 + 光标
                    if self.text_editing {
                        if let Some(ann) = self.annotations.last() {
                            draw_text_edit_box(
                                &mut buffer[..],
                                w.get(),
                                h.get(),
                                ann,
                                &self.ime_preedit,
                                self.cursor_visible,
                            );
                        }
                    }
                    draw_toolbar(
                        &mut buffer[..],
                        w.get(),
                        h.get(),
                        self.sel,
                        self.tool,
                        self.color,
                        self.toolbar_hover,
                        self.palette_open,
                    );
                    if self.palette_open {
                        draw_palette_popup(
                            &mut buffer[..],
                            w.get(),
                            h.get(),
                            self.sel,
                            self.color,
                            self.palette_hover,
                        );
                    }
                } else if self.mode == Mode::Pinned {
                    draw_pin_controls(&mut buffer[..], w.get(), h.get());
                } else {
                    draw_select_badge(&mut buffer[..], w.get(), h.get());
                }
                buffer.present().unwrap();
            }
            _ => (),
        }
    }
}

/// EnumWindows 回调：把可见、未最小化的顶层窗口矩形收集进 lparam 指向的 Vec。
unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<RECT>) };
    unsafe {
        // 只要可见、没最小化的
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return BOOL(1);
        }
        // 跳过被 DWM 隐藏（cloaked）的幽灵窗口：UWP 隐形窗等，可见却看不到
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            4,
        );
        if cloaked != 0 {
            return BOOL(1);
        }
        // 取真实可视边界（不含阴影）；DWM 拿不到就退回 GetWindowRect
        let mut r = RECT::default();
        let dwm_ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok();
        if !dwm_ok && GetWindowRect(hwnd, &mut r).is_err() {
            return BOOL(1);
        }
        // 太小的（1×1 幽灵、图标窗）跳过
        if r.right - r.left >= 40 && r.bottom - r.top >= 40 {
            list.push(r);
        }
    }
    BOOL(1) // TRUE = 继续枚举
}

/// 在像素缓冲上画空心矩形边框，`t` 是线的粗细（像素）。color 是 0RGB 的 u32。
fn draw_rect(
    buf: &mut [u32],
    w: u32,
    h: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    t: i32,
) {
    let (w, h) = (w as i32, h as i32);
    let left = x0.min(x1).clamp(0, w - 1);
    let right = x0.max(x1).clamp(0, w - 1);
    let top = y0.min(y1).clamp(0, h - 1);
    let bottom = y0.max(y1).clamp(0, h - 1);
    let t = t.max(1);
    // 上、下两条横边（各 t 像素厚，往内叠）
    for d in 0..t {
        let yt = (top + d).min(h - 1);
        let yb = (bottom - d).max(0);
        for x in left..=right {
            buf[(yt * w + x) as usize] = color;
            buf[(yb * w + x) as usize] = color;
        }
    }
    // 左、右两条竖边
    for d in 0..t {
        let xl = (left + d).min(w - 1);
        let xr = (right - d).max(0);
        for y in top..=bottom {
            buf[(y * w + xl) as usize] = color;
            buf[(y * w + xr) as usize] = color;
        }
    }
}

const TOOLBAR_HEIGHT: i32 = 38;
const TOOLBAR_GAP: i32 = 4;
const SWATCH: i32 = 26; // 色板色块边长
const SWATCH_GAP: i32 = 4;
const PALETTE_PAD: i32 = 6; // 色板弹层内边距

// 单行工具栏：PEN / LINE / RECT / TEXT / COLOR / UNDO / COPY / OCR / PIN / SELECT / X
const TOOLBAR_ITEM_WIDTHS: [i32; 11] = [46, 50, 50, 50, 44, 50, 50, 42, 44, 74, 30];
const TOOLBAR_SLOT_COUNT: usize = 11;
const TOOLBAR_SLOT_COLOR: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolbarItem {
    Tool(Tool),
    /// 色板按钮：点击开关二级色板菜单
    Color,
    Action(ToolbarAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolbarAction {
    Copy,
    Ocr,
    Reselect,
    Pin,
    Undo,
    Close,
}

fn toolbar_item(slot: usize) -> ToolbarItem {
    match slot {
        0 => ToolbarItem::Tool(Tool::Pen),
        1 => ToolbarItem::Tool(Tool::Line),
        2 => ToolbarItem::Tool(Tool::Rect),
        3 => ToolbarItem::Tool(Tool::Text),
        4 => ToolbarItem::Color,
        5 => ToolbarItem::Action(ToolbarAction::Undo),
        6 => ToolbarItem::Action(ToolbarAction::Copy),
        7 => ToolbarItem::Action(ToolbarAction::Ocr),
        8 => ToolbarItem::Action(ToolbarAction::Pin),
        9 => ToolbarItem::Action(ToolbarAction::Reselect),
        _ => ToolbarItem::Action(ToolbarAction::Close),
    }
}

fn toolbar_item_slot(item: ToolbarItem) -> usize {
    match item {
        ToolbarItem::Tool(Tool::Pen) => 0,
        ToolbarItem::Tool(Tool::Line) => 1,
        ToolbarItem::Tool(Tool::Rect) => 2,
        ToolbarItem::Tool(Tool::Text) => 3,
        ToolbarItem::Color => 4,
        ToolbarItem::Action(ToolbarAction::Undo) => 5,
        ToolbarItem::Action(ToolbarAction::Copy) => 6,
        ToolbarItem::Action(ToolbarAction::Ocr) => 7,
        ToolbarItem::Action(ToolbarAction::Pin) => 8,
        ToolbarItem::Action(ToolbarAction::Reselect) => 9,
        ToolbarItem::Action(ToolbarAction::Close) => 10,
    }
}

fn toolbar_size() -> (i32, i32) {
    let w = TOOLBAR_ITEM_WIDTHS.iter().sum::<i32>() + TOOLBAR_GAP * (TOOLBAR_SLOT_COUNT as i32 - 1);
    (w, TOOLBAR_HEIGHT)
}

fn toolbar_origin(w: i32, h: i32, sel: Option<((i32, i32), (i32, i32))>) -> (i32, i32) {
    let (tw, th) = toolbar_size();
    let (left, top, right, bottom) =
        sel.map(normalized_rect)
            .unwrap_or((w / 2 - 1, h / 2 - 1, w / 2 + 1, h / 2 + 1));
    let max_x = (w - tw - 8).max(8);
    let x = ((left + right - tw) / 2).clamp(8, max_x);
    let y = if bottom + th + 8 <= h {
        bottom + 8
    } else {
        (top - th - 8).max(8)
    };
    (x, y.clamp(8, (h - th - 8).max(8)))
}

fn toolbar_item_rect(origin: (i32, i32), slot: usize) -> (i32, i32, i32, i32) {
    let mut x = origin.0;
    for i in 0..slot {
        x += TOOLBAR_ITEM_WIDTHS[i] + TOOLBAR_GAP;
    }
    (
        x,
        origin.1,
        x + TOOLBAR_ITEM_WIDTHS[slot],
        origin.1 + TOOLBAR_HEIGHT,
    )
}

fn toolbar_hit(
    p: (i32, i32),
    w: i32,
    h: i32,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<ToolbarItem> {
    let origin = toolbar_origin(w, h, sel);
    (0..TOOLBAR_SLOT_COUNT).find_map(|slot| {
        let (x0, y0, x1, y1) = toolbar_item_rect(origin, slot);
        (p.0 >= x0 && p.0 < x1 && p.1 >= y0 && p.1 < y1).then(|| toolbar_item(slot))
    })
}

/// 色板弹层的整体矩形（对齐在色板按钮下，水平居中）。
fn palette_size() -> (i32, i32) {
    let w =
        PALETTE.len() as i32 * SWATCH + (PALETTE.len() as i32 - 1) * SWATCH_GAP + PALETTE_PAD * 2;
    (w, SWATCH + PALETTE_PAD * 2)
}

fn palette_popup_rect(w: i32, h: i32, color_rect: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let (pw, ph) = palette_size();
    let cx = (color_rect.0 + color_rect.2) / 2;
    let x = (cx - pw / 2).clamp(8, (w - pw - 8).max(8));
    let below = color_rect.3 + TOOLBAR_GAP;
    let y = if below + ph <= h {
        below
    } else {
        (color_rect.1 - ph - TOOLBAR_GAP).max(8)
    };
    (x, y, x + pw, y + ph)
}

fn palette_swatch_rect(popup: (i32, i32, i32, i32), i: usize) -> (i32, i32, i32, i32) {
    let x = popup.0 + PALETTE_PAD + i as i32 * (SWATCH + SWATCH_GAP);
    (
        x,
        popup.1 + PALETTE_PAD,
        x + SWATCH,
        popup.1 + PALETTE_PAD + SWATCH,
    )
}

fn palette_hit(
    p: (i32, i32),
    w: i32,
    h: i32,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<usize> {
    let origin = toolbar_origin(w, h, sel);
    let color_rect = toolbar_item_rect(origin, TOOLBAR_SLOT_COLOR);
    let popup = palette_popup_rect(w, h, color_rect);
    (0..PALETTE.len()).find_map(|i| {
        let (x0, y0, x1, y1) = palette_swatch_rect(popup, i);
        (p.0 >= x0 && p.0 < x1 && p.1 >= y0 && p.1 < y1).then_some(i)
    })
}

fn draw_fill_rect(buf: &mut [u32], w: u32, h: u32, rect: (i32, i32, i32, i32), color: u32) {
    let (x0, y0, x1, y1) = rect;
    let left = x0.clamp(0, w as i32);
    let top = y0.clamp(0, h as i32);
    let right = x1.clamp(0, w as i32);
    let bottom = y1.clamp(0, h as i32);
    for y in top..bottom {
        let start = (y as u32 * w + left as u32) as usize;
        let end = (y as u32 * w + right as u32) as usize;
        if start < end && end <= buf.len() {
            buf[start..end].fill(color);
        }
    }
}

fn draw_toolbar(
    buf: &mut [u32],
    w: u32,
    h: u32,
    sel: Option<((i32, i32), (i32, i32))>,
    tool: Tool,
    color: [u8; 4],
    hover: Option<usize>,
    palette_open: bool,
) {
    let origin = toolbar_origin(w as i32, h as i32, sel);
    let (tw, th) = toolbar_size();
    draw_fill_rect(
        buf,
        w,
        h,
        (
            origin.0 + 2,
            origin.1 + 3,
            origin.0 + tw + 2,
            origin.1 + th + 3,
        ),
        0x00101010,
    );
    draw_fill_rect(
        buf,
        w,
        h,
        (origin.0, origin.1, origin.0 + tw, origin.1 + th),
        0x00212631,
    );
    for slot in 0..TOOLBAR_SLOT_COUNT {
        let rect = toolbar_item_rect(origin, slot);
        let item = toolbar_item(slot);
        let mut fill = match item {
            ToolbarItem::Tool(t) => {
                if t == tool {
                    0x00D88928
                } else {
                    0x004B5968
                }
            }
            ToolbarItem::Color => 0x003B78C8,
            ToolbarItem::Action(ToolbarAction::Copy) => 0x002D9B68,
            ToolbarItem::Action(ToolbarAction::Ocr) => 0x00704AA3,
            ToolbarItem::Action(ToolbarAction::Reselect) => 0x006B5AA8,
            ToolbarItem::Action(ToolbarAction::Pin) => 0x003B78C8,
            ToolbarItem::Action(ToolbarAction::Undo) => 0x00515D6B,
            ToolbarItem::Action(ToolbarAction::Close) => 0x00A83D48,
        };
        if hover == Some(slot) {
            fill = match item {
                ToolbarItem::Action(ToolbarAction::Close) => 0x00D85A65,
                _ => 0x006D91B5,
            };
        }
        draw_fill_rect(buf, w, h, rect, fill);
        // 高亮：选中的工具 / 打开中的色板按钮
        match item {
            ToolbarItem::Tool(t) if t == tool => {
                draw_rect(
                    buf,
                    w,
                    h,
                    rect.0,
                    rect.1,
                    rect.2 - 1,
                    rect.3 - 1,
                    0x00FFFFFF,
                    2,
                );
            }
            ToolbarItem::Color if palette_open => {
                draw_rect(
                    buf,
                    w,
                    h,
                    rect.0,
                    rect.1,
                    rect.2 - 1,
                    rect.3 - 1,
                    0x00FFFFFF,
                    2,
                );
            }
            _ => {}
        }
        draw_rect(
            buf,
            w,
            h,
            rect.0,
            rect.1,
            rect.2 - 1,
            rect.3 - 1,
            0x00D9E2EC,
            1,
        );
        match item {
            // 色板按钮：居中画当前颜色方块 + 下方下拉箭头
            ToolbarItem::Color => {
                let s = 18;
                let sx = rect.0 + (rect.2 - rect.0 - s) / 2;
                let sy = rect.1 + 7;
                draw_fill_rect(buf, w, h, (sx, sy, sx + s, sy + s), color_u32(color));
                draw_rect(buf, w, h, sx, sy, sx + s - 1, sy + s - 1, 0x00FFFFFF, 1);
                let cx = (rect.0 + rect.2) / 2;
                let ay = rect.3 - 9;
                for d in 0..4 {
                    let half = 3 - d;
                    draw_fill_rect(
                        buf,
                        w,
                        h,
                        (cx - half, ay + d, cx + half + 1, ay + d + 1),
                        0x00FFFFFF,
                    );
                }
            }
            _ => {
                let label = match item {
                    ToolbarItem::Tool(Tool::Pen) => "PEN",
                    ToolbarItem::Tool(Tool::Line) => "LINE",
                    ToolbarItem::Tool(Tool::Rect) => "RECT",
                    ToolbarItem::Tool(Tool::Text) => "TEXT",
                    ToolbarItem::Action(ToolbarAction::Copy) => "COPY",
                    ToolbarItem::Action(ToolbarAction::Ocr) => "OCR",
                    ToolbarItem::Action(ToolbarAction::Reselect) => "SELECT",
                    ToolbarItem::Action(ToolbarAction::Pin) => "PIN",
                    ToolbarItem::Action(ToolbarAction::Undo) => "UNDO",
                    ToolbarItem::Action(ToolbarAction::Close) => "X",
                    _ => "",
                };
                let text_width = (label.chars().count() as i32 * 12) - 2;
                draw_text(
                    buf,
                    w,
                    h,
                    rect.0 + (rect.2 - rect.0 - text_width) / 2,
                    rect.1 + 10,
                    label,
                    2,
                    0x00FFFFFF,
                );
            }
        }
    }
}

/// 画打开的二级色板菜单：8 个色块一排，当前颜色白框、悬停蓝框。
fn draw_palette_popup(
    buf: &mut [u32],
    w: u32,
    h: u32,
    sel: Option<((i32, i32), (i32, i32))>,
    color: [u8; 4],
    hover: Option<usize>,
) {
    let origin = toolbar_origin(w as i32, h as i32, sel);
    let color_rect = toolbar_item_rect(origin, TOOLBAR_SLOT_COLOR);
    let popup = palette_popup_rect(w as i32, h as i32, color_rect);
    draw_fill_rect(
        buf,
        w,
        h,
        (popup.0 + 2, popup.1 + 3, popup.2 + 2, popup.3 + 3),
        0x00101010,
    );
    draw_fill_rect(buf, w, h, popup, 0x00212631);
    for i in 0..PALETTE.len() {
        let rect = palette_swatch_rect(popup, i);
        draw_fill_rect(buf, w, h, rect, color_u32(PALETTE[i]));
        if PALETTE[i] == color {
            draw_rect(
                buf,
                w,
                h,
                rect.0 - 1,
                rect.1 - 1,
                rect.2,
                rect.3,
                0x00FFFFFF,
                2,
            );
        } else if hover == Some(i) {
            draw_rect(
                buf,
                w,
                h,
                rect.0 - 1,
                rect.1 - 1,
                rect.2,
                rect.3,
                0x006D91B5,
                2,
            );
        }
        draw_rect(
            buf,
            w,
            h,
            rect.0,
            rect.1,
            rect.2 - 1,
            rect.3 - 1,
            0x00D9E2EC,
            1,
        );
    }
}

fn draw_text(buf: &mut [u32], w: u32, h: u32, x: i32, y: i32, text: &str, scale: i32, color: u32) {
    let mut cursor_x = x;
    for ch in text.chars() {
        let rows = glyph(ch);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    draw_fill_rect(
                        buf,
                        w,
                        h,
                        (
                            cursor_x + col * scale,
                            y + row as i32 * scale,
                            cursor_x + (col + 1) * scale,
                            y + (row as i32 + 1) * scale,
                        ),
                        color,
                    );
                }
            }
        }
        cursor_x += 5 * scale + scale;
    }
}

fn glyph(ch: char) -> [u8; 7] {
    match ch {
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'N' => [
            0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        _ => [0; 7],
    }
}

fn pin_close_rect(w: i32, _h: i32) -> (i32, i32, i32, i32) {
    ((w - 38).max(8), 8, (w - 8).max(30), 36)
}

fn draw_pin_controls(buf: &mut [u32], w: u32, h: u32) {
    let close = pin_close_rect(w as i32, h as i32);
    draw_fill_rect(buf, w, h, (8, 8, 50, 36), 0x003B78C8);
    draw_text(buf, w, h, 14, 18, "PIN", 2, 0x00FFFFFF);
    draw_fill_rect(buf, w, h, close, 0x00A83D48);
    draw_rect(
        buf,
        w,
        h,
        close.0,
        close.1,
        close.2 - 1,
        close.3 - 1,
        0x00FFFFFF,
        1,
    );
    draw_text(buf, w, h, close.0 + 9, close.1 + 10, "X", 2, 0x00FFFFFF);
}

fn draw_select_badge(buf: &mut [u32], w: u32, h: u32) {
    let rect = (8, 8, 108, 36);
    draw_fill_rect(buf, w, h, rect, 0x003B78C8);
    draw_rect(
        buf,
        w,
        h,
        rect.0,
        rect.1,
        rect.2 - 1,
        rect.3 - 1,
        0x00FFFFFF,
        1,
    );
    draw_text(buf, w, h, 16, 18, "SELECT", 2, 0x00FFFFFF);
}

fn shade_outside(buf: &mut [u32], w: u32, h: u32, a: (i32, i32), b: (i32, i32)) {
    let (left, top, right, bottom) = normalized_rect((a, b));
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if x < left || x > right || y < top || y > bottom {
                let i = (y as u32 * w + x as u32) as usize;
                let c = buf[i];
                buf[i] = ((c & 0x00FEFEFE) >> 1) & 0x007F7F7F;
            }
        }
    }
}

fn normalized_rect((a, b): ((i32, i32), (i32, i32))) -> (i32, i32, i32, i32) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

fn point_in_selection(p: (i32, i32), sel: Option<((i32, i32), (i32, i32))>) -> bool {
    sel.map(normalized_rect)
        .is_some_and(|r| p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3)
}

fn draw_line_buffer(
    buf: &mut [u32],
    w: u32,
    h: u32,
    a: (i32, i32),
    b: (i32, i32),
    color: u32,
    radius: i32,
) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let steps = dx.abs().max(dy.abs()).max(1);
    for i in 0..=steps {
        let x = a.0 + dx * i / steps;
        let y = a.1 + dy * i / steps;
        for oy in -radius..=radius {
            for ox in -radius..=radius {
                if ox * ox + oy * oy <= radius * radius {
                    let (px, py) = (x + ox, y + oy);
                    if px >= 0 && py >= 0 && px < w as i32 && py < h as i32 {
                        buf[(py as u32 * w + px as u32) as usize] = color;
                    }
                }
            }
        }
    }
}

fn draw_line_image(img: &mut RgbaImage, a: (i32, i32), b: (i32, i32), color: [u8; 4], radius: i32) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let steps = dx.abs().max(dy.abs()).max(1);
    for i in 0..=steps {
        let x = a.0 + dx * i / steps;
        let y = a.1 + dy * i / steps;
        for oy in -radius..=radius {
            for ox in -radius..=radius {
                let (px, py) = (x + ox, y + oy);
                if ox * ox + oy * oy <= radius * radius
                    && px >= 0
                    && py >= 0
                    && px < img.width() as i32
                    && py < img.height() as i32
                {
                    img.put_pixel(px as u32, py as u32, xcap::image::Rgba(color));
                }
            }
        }
    }
}

/// 直角矩形边框（画进 RGBA 图，供输出用）。t 是边框粗细。
fn draw_rect_image(img: &mut RgbaImage, a: (i32, i32), b: (i32, i32), color: [u8; 4], t: i32) {
    let (left, top, right, bottom) = normalized_rect((a, b));
    for d in 0..t {
        draw_line_image(img, (left, top + d), (right, top + d), color, 0);
        draw_line_image(img, (left, bottom - d), (right, bottom - d), color, 0);
        draw_line_image(img, (left + d, top), (left + d, bottom), color, 0);
        draw_line_image(img, (right - d, top), (right - d, bottom), color, 0);
    }
}

/// RGBA 颜色 → 显示缓冲用的 0RGB u32。
fn color_u32(c: [u8; 4]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// 画笔/直线的线宽（半径，直径 = 2*radius+1 = 3px），与矩形边框 t=3 保持一致。
const ANNOT_LINE_T: i32 = 1;

/// 文字标注的字体高度（物理像素）。
const TEXT_FONT_HEIGHT: i32 = 20;

/// 建文字标注用的字体：微软雅黑（覆盖中文），负高度 = 按像素。
unsafe fn create_text_font() -> windows::Win32::Graphics::Gdi::HFONT {
    unsafe {
        CreateFontW(
            -TEXT_FONT_HEIGHT,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 as u32) | (FF_DONTCARE.0 as u32),
            w!("Microsoft YaHei"),
        )
    }
}

/// 用 GDI 量出文字尺寸（DT_CALCRECT，与渲染一致）。空串直接返回最小尺寸，避免把空切片交给 DrawTextW 读越界。
fn gdi_text_size(text: &str) -> (i32, i32) {
    if text.is_empty() {
        return (1, TEXT_FONT_HEIGHT);
    }
    let mut wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return (0, 0);
        }
        let font = create_text_font();
        if font.is_invalid() {
            let _ = DeleteDC(hdc);
            return (0, 0);
        }
        let prev_font = SelectObject(hdc, HGDIOBJ(font.0));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let _ = DrawTextW(
            hdc,
            &mut wide,
            &mut rect,
            DT_LEFT | DT_TOP | DT_NOPREFIX | DT_CALCRECT,
        );
        let size = (rect.right.max(1), rect.bottom.max(1));
        SelectObject(hdc, prev_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteDC(hdc);
        size
    }
}

/// 用 GDI 把文字画成 RGBA 像素：白字黑底取覆盖率（灰度抗锯齿），再按目标色染色。
/// 返回 (宽, 高, RGBA)。空串返回 None。
fn gdi_render_text_rgba(text: &str, color: [u8; 4]) -> Option<(i32, i32, Vec<u8>)> {
    if text.is_empty() {
        return None;
    }
    let mut wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let font = create_text_font();
        if font.is_invalid() {
            let _ = DeleteDC(hdc);
            return None;
        }
        let prev_font = SelectObject(hdc, HGDIOBJ(font.0));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let _ = DrawTextW(
            hdc,
            &mut wide,
            &mut rect,
            DT_LEFT | DT_TOP | DT_NOPREFIX | DT_CALCRECT,
        );
        let (tw, th) = (rect.right.max(1), rect.bottom.max(1));
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = tw;
        bmi.bmiHeader.biHeight = -th; // 负值 = 顶向下，与图像行序一致
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;
        let mut bits: *mut c_void = std::ptr::null_mut();
        let hbmp = match CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(h) => h,
            Err(_) => {
                SelectObject(hdc, prev_font);
                let _ = DeleteObject(HGDIOBJ(font.0));
                let _ = DeleteDC(hdc);
                return None;
            }
        };
        let prev_bmp = SelectObject(hdc, HGDIOBJ(hbmp.0));
        SetBkMode(hdc, OPAQUE);
        SetBkColor(hdc, COLORREF(0)); // 黑底
        SetTextColor(hdc, COLORREF(0x00FFFFFF)); // 白字
        let _ = DrawTextW(hdc, &mut wide, &mut rect, DT_LEFT | DT_TOP | DT_NOPREFIX);
        let mut raw = vec![0u8; (tw * th * 4) as usize];
        let _ = GetDIBits(
            hdc,
            hbmp,
            0,
            th as u32,
            Some(raw.as_mut_ptr() as *mut c_void),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(hdc, prev_bmp);
        SelectObject(hdc, prev_font);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteDC(hdc);
        // BGRA(top-down) → RGBA，覆盖率 = 灰度亮度，按目标色染色
        let mut rgba = vec![0u8; raw.len()];
        for (dst, src) in rgba.chunks_exact_mut(4).zip(raw.chunks_exact(4)) {
            let cov = ((src[0] as u32 + src[1] as u32 + src[2] as u32) / 3) as u8;
            dst[0] = (color[0] as u32 * cov as u32 / 255) as u8;
            dst[1] = (color[1] as u32 * cov as u32 / 255) as u8;
            dst[2] = (color[2] as u32 * cov as u32 / 255) as u8;
            dst[3] = cov;
        }
        Some((tw, th, rgba))
    }
}

/// 把 RGBA 子图以 source-over 混合到显示缓冲（0RGB，无 alpha）。
fn blend_rgba_buffer(
    buf: &mut [u32],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    rgba: &[u8],
    tw: i32,
    th: i32,
) {
    for j in 0..th {
        for i in 0..tw {
            let idx = ((j * tw + i) * 4) as usize;
            let a = rgba[idx + 3] as u32;
            if a == 0 {
                continue;
            }
            let (px, py) = (x + i, y + j);
            if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                continue;
            }
            let dst = buf[(py as u32 * w + px as u32) as usize];
            let inv = 255 - a;
            let or = (rgba[idx] as u32 * a + ((dst >> 16) & 0xFF) * inv) / 255;
            let og = (rgba[idx + 1] as u32 * a + ((dst >> 8) & 0xFF) * inv) / 255;
            let ob = (rgba[idx + 2] as u32 * a + (dst & 0xFF) * inv) / 255;
            buf[(py as u32 * w + px as u32) as usize] = (or << 16) | (og << 8) | ob;
        }
    }
}

/// 把 RGBA 子图以 source-over 混合到 RGBA 图。
fn blend_rgba_image(img: &mut RgbaImage, x: i32, y: i32, rgba: &[u8], tw: i32, th: i32) {
    for j in 0..th {
        for i in 0..tw {
            let idx = ((j * tw + i) * 4) as usize;
            let a = rgba[idx + 3] as u32;
            if a == 0 {
                continue;
            }
            let (px, py) = (x + i, y + j);
            if px < 0 || py < 0 || px >= img.width() as i32 || py >= img.height() as i32 {
                continue;
            }
            let dst = img.get_pixel(px as u32, py as u32).0;
            let inv = 255 - a;
            let or = (rgba[idx] as u32 * a + dst[0] as u32 * inv) / 255;
            let og = (rgba[idx + 1] as u32 * a + dst[1] as u32 * inv) / 255;
            let ob = (rgba[idx + 2] as u32 * a + dst[2] as u32 * inv) / 255;
            img.put_pixel(
                px as u32,
                py as u32,
                xcap::image::Rgba([or as u8, og as u8, ob as u8, 255]),
            );
        }
    }
}

fn draw_text_buffer(buf: &mut [u32], w: u32, h: u32, text: &str, pos: (i32, i32), color: [u8; 4]) {
    if let Some((tw, th, rgba)) = gdi_render_text_rgba(text, color) {
        blend_rgba_buffer(buf, w, h, pos.0, pos.1, &rgba, tw, th);
    }
}

fn draw_text_image(img: &mut RgbaImage, text: &str, pos: (i32, i32), color: [u8; 4]) {
    if let Some((tw, th, rgba)) = gdi_render_text_rgba(text, color) {
        blend_rgba_image(img, pos.0, pos.1, &rgba, tw, th);
    }
}

/// 画文字输入提示：组合拼音（浅色+下划线）+ 闪烁光标。不画外边框。
fn draw_text_edit_box(
    buf: &mut [u32],
    w: u32,
    h: u32,
    ann: &Annotation,
    preedit: &str,
    cursor_visible: bool,
) {
    const CARET_COLOR: u32 = 0x004C9AFF; // 亮蓝：在任何底色上都醒目
    if let Shape::Text(pos, text) = &ann.shape {
        let full = format!("{text}{preedit}");
        let (tw, th) = gdi_text_size(&full);
        let (tw, th) = (tw.max(4), th.max(TEXT_FONT_HEIGHT));
        // 组合中的拼音：画在已提交文字后面，用浅色 + 下划线区分
        if !preedit.is_empty() {
            let (tw0, _) = gdi_text_size(text);
            let lighter = [
                ann.color[0] + (255 - ann.color[0]) / 2,
                ann.color[1] + (255 - ann.color[1]) / 2,
                ann.color[2] + (255 - ann.color[2]) / 2,
                255,
            ];
            draw_text_buffer(buf, w, h, preedit, (pos.0 + tw0, pos.1), lighter);
            draw_line_buffer(
                buf,
                w,
                h,
                (pos.0 + tw0, pos.1 + th - 2),
                (pos.0 + tw, pos.1 + th - 2),
                color_u32(lighter),
                0,
            );
        }
        // 闪烁光标：3px 宽实心竖条，紧跟文字末尾
        if cursor_visible {
            let cx = pos.0 + tw;
            draw_line_buffer(
                buf,
                w,
                h,
                (cx, pos.1 + 2),
                (cx, pos.1 + th - 2),
                CARET_COLOR,
                1,
            );
        }
    }
}

/// 把一条标注画到显示缓冲（窗口内坐标）。
fn draw_annotation_buffer(buf: &mut [u32], w: u32, h: u32, ann: &Annotation) {
    let c = color_u32(ann.color);
    match &ann.shape {
        Shape::Pen(points) => {
            for pair in points.windows(2) {
                draw_line_buffer(buf, w, h, pair[0], pair[1], c, ANNOT_LINE_T);
            }
        }
        Shape::Line(a, b) => draw_line_buffer(buf, w, h, *a, *b, c, ANNOT_LINE_T),
        Shape::Rect(a, b) => draw_rect(buf, w, h, a.0, a.1, b.0, b.1, c, 3),
        Shape::Text(pos, text) => draw_text_buffer(buf, w, h, text, *pos, ann.color),
    }
}

/// 把一条标注画到输出图（坐标按选区偏移换算）。
fn draw_annotation_image(img: &mut RgbaImage, ann: &Annotation, offset: (i32, i32)) {
    let o = offset;
    match &ann.shape {
        Shape::Pen(points) => {
            for pair in points.windows(2) {
                draw_line_image(
                    img,
                    (pair[0].0 - o.0, pair[0].1 - o.1),
                    (pair[1].0 - o.0, pair[1].1 - o.1),
                    ann.color,
                    ANNOT_LINE_T,
                );
            }
        }
        Shape::Line(a, b) => draw_line_image(
            img,
            (a.0 - o.0, a.1 - o.1),
            (b.0 - o.0, b.1 - o.1),
            ann.color,
            ANNOT_LINE_T,
        ),
        Shape::Rect(a, b) => draw_rect_image(
            img,
            (a.0 - o.0, a.1 - o.1),
            (b.0 - o.0, b.1 - o.1),
            ann.color,
            3,
        ),
        Shape::Text(pos, text) => draw_text_image(img, text, (pos.0 - o.0, pos.1 - o.1), ann.color),
    }
}

/// 代码画一个"取景框"图标：透明底 + 四个角标（截图/框选的通用意象）。
fn make_icon() -> Icon {
    const N: i32 = 64; // 画大点，Windows 缩小到 16/32 更清晰
    const M: i32 = 10; // 边距
    const T: i32 = 6; // 线粗
    const L: i32 = 22; // 每条臂的长度
    let color = [0x4Cu8, 0x9A, 0xFF, 0xFF]; // 亮蓝，深浅任务栏都看得见

    let mut px = vec![0u8; (N * N * 4) as usize]; // 全透明底
    // 8 条臂：每个角一横一竖，(x0,y0,x1,y1) 半开区间
    let arms: [(i32, i32, i32, i32); 8] = [
        (M, M, M + L, M + T),                 // 左上 横
        (M, M, M + T, M + L),                 // 左上 竖
        (N - M - L, M, N - M, M + T),         // 右上 横
        (N - M - T, M, N - M, M + L),         // 右上 竖
        (M, N - M - T, M + L, N - M),         // 左下 横
        (M, N - M - L, M + T, N - M),         // 左下 竖
        (N - M - L, N - M - T, N - M, N - M), // 右下 横
        (N - M - T, N - M - L, N - M, N - M), // 右下 竖
    ];
    for (x0, y0, x1, y1) in arms {
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * N + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&color);
            }
        }
    }
    Icon::from_rgba(px, N as u32, N as u32).unwrap()
}

/// 按对角两点 a、b 从原图裁出子矩形，进剪贴板。零尺寸就跳过。
fn crop_image(img: &RgbaImage, a: (i32, i32), b: (i32, i32)) -> Option<RgbaImage> {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let left = a.0.min(b.0).clamp(0, iw);
    let right = a.0.max(b.0).clamp(0, iw);
    let top = a.1.min(b.1).clamp(0, ih);
    let bottom = a.1.max(b.1).clamp(0, ih);
    let (bw, bh) = ((right - left) as u32, (bottom - top) as u32);
    if bw == 0 || bh == 0 {
        return None;
    }
    Some(imageops::crop_imm(img, left as u32, top as u32, bw, bh).to_image())
}

/// 计算 OCR 使用的原图区域。返回 left / top / width / height，全部限制在图片内。
fn ocr_region(
    img: &RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<(u32, u32, u32, u32)> {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let (left, top, right, bottom) = sel.map(normalized_rect).unwrap_or((0, 0, iw, ih));
    let left = left.clamp(0, iw);
    let right = right.clamp(0, iw);
    let top = top.clamp(0, ih);
    let bottom = bottom.clamp(0, ih);
    (right > left && bottom > top).then_some((
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// 从原始截图提取 OCR 输入，并在超过系统上限时等比缩小。这样不会识别用户画的标注。
fn prepare_ocr_rgba<'a>(
    img: &'a RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
    max_dimension: u32,
) -> Option<(Cow<'a, [u8]>, u32, u32)> {
    if max_dimension == 0 {
        return None;
    }
    let (left, top, width, height) = ocr_region(img, sel)?;
    let largest = width.max(height);
    if largest > max_dimension {
        let scaled_width = ((width as u64 * max_dimension as u64) / largest as u64).max(1) as u32;
        let scaled_height = ((height as u64 * max_dimension as u64) / largest as u64).max(1) as u32;
        let view = imageops::crop_imm(img, left, top, width, height);
        let resized = imageops::resize(
            &*view,
            scaled_width,
            scaled_height,
            imageops::FilterType::Triangle,
        );
        return Some((Cow::Owned(resized.into_raw()), scaled_width, scaled_height));
    }

    if left == 0 && top == 0 && width == img.width() && height == img.height() {
        return Some((Cow::Borrowed(img.as_raw()), width, height));
    }

    // 未缩放时按行直接复制选区，避免先创建一个 RgbaImage 再复制进 WinRT 缓冲区。
    let source_stride = img.width() as usize * 4;
    let row_bytes = width as usize * 4;
    let raw = img.as_raw();
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    for y in top..top + height {
        let start = y as usize * source_stride + left as usize * 4;
        rgba.extend_from_slice(&raw[start..start + row_bytes]);
    }
    Some((Cow::Owned(rgba), width, height))
}

/// 调用系统自带的 Windows.Media.Ocr，按用户系统语言识别，不需要联网或随程序携带模型。
fn recognize_image_text(
    img: &RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Result<String, String> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("无法创建系统 OCR 引擎（请确认已安装对应语言包）：{e}"))?;
    let max_dimension =
        OcrEngine::MaxImageDimension().map_err(|e| format!("无法读取 OCR 图片尺寸上限：{e}"))?;
    let (rgba, width, height) =
        prepare_ocr_rgba(img, sel, max_dimension).ok_or_else(|| String::from("选区尺寸无效"))?;

    let writer = DataWriter::new().map_err(|e| format!("无法创建图片缓冲区：{e}"))?;
    writer
        .WriteBytes(&rgba)
        .map_err(|e| format!("无法写入图片缓冲区：{e}"))?;
    let buffer = writer
        .DetachBuffer()
        .map_err(|e| format!("无法读取图片缓冲区：{e}"))?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Rgba8,
        width as i32,
        height as i32,
        BitmapAlphaMode::Straight,
    )
    .map_err(|e| format!("无法创建 OCR 图片：{e}"))?;
    drop(buffer);
    drop(writer);
    drop(rgba);

    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|operation| operation.join())
        .map_err(|e| format!("系统 OCR 识别失败：{e}"))?;
    let text = result
        .Text()
        .map_err(|e| format!("无法读取 OCR 结果：{e}"))?
        .to_string();
    Ok(text.trim().to_owned())
}

fn unicode_text_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn text_to_clipboard(text: &str) -> bool {
    let bytes = unicode_text_bytes(text);
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        if EmptyClipboard().is_err() {
            let _ = CloseClipboard();
            return false;
        }
        let success = global_from_bytes(&bytes).is_some_and(|h| {
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))).is_ok() {
                true
            } else {
                let _ = GlobalFree(Some(h));
                false
            }
        });
        let _ = CloseClipboard();
        success
    }
}

/// 把截图放进剪贴板，同时挂两种格式：
/// - CF_DIB 位图：微信/Word/画图 等能贴图的程序直接粘。
/// - CF_HDROP 文件：把图另存成临时 png，终端/资源管理器粘到的是这个文件路径。
fn image_to_clipboard(img: &RgbaImage) {
    // 存一份临时 png，好让只认文件的地方（命令行）也能粘到路径
    let png_path = std::env::temp_dir().join("rshot.png");
    let hdrop = img.save(&png_path).ok().map(|_| build_hdrop(&png_path));
    // PNG 编码完成后再构造 DIB，避免两份大块临时数据同时参与编码峰值。
    let dib = build_dib(img);

    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Some(h) = global_from_bytes(&dib) {
            if SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(h.0))).is_err() {
                let _ = GlobalFree(Some(h));
            }
        }
        if let Some(bytes) = hdrop {
            if let Some(h) = global_from_bytes(&bytes) {
                if SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(h.0))).is_err() {
                    let _ = GlobalFree(Some(h));
                }
            }
        }
        let _ = CloseClipboard();
    }
}

/// 组一个 24 位 BI_RGB 的 DIB：40 字节 BITMAPINFOHEADER + 自底向上、每行补齐 4 字节的 BGR 像素。
fn build_dib(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let stride = (w * 3 + 3) & !3; // 每行补齐到 4 字节边界（DIB 要求）
    let mut out = Vec::with_capacity(40 + stride * h);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(w as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&(h as i32).to_le_bytes()); // biHeight 正=自底向上
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&24u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&((stride * h) as u32).to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for y in (0..h).rev() {
        let mut row = 0usize;
        for x in 0..w {
            let px = img.get_pixel(x as u32, y as u32).0;
            out.push(px[2]); // B
            out.push(px[1]); // G
            out.push(px[0]); // R
            row += 3;
        }
        while row < stride {
            out.push(0); // 行尾补齐
            row += 1;
        }
    }
    out
}

/// 组一个 CF_HDROP 数据块：DROPFILES 头 + 宽字符路径 + 双 null 结尾。
fn build_hdrop(path: &std::path::Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let df = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32, // 路径列表相对本头的偏移
        pt: POINT { x: 0, y: 0 },
        fNC: BOOL(0),
        fWide: BOOL(1), // 宽字符路径
    };
    let head = unsafe {
        std::slice::from_raw_parts(
            (&df as *const DROPFILES) as *const u8,
            std::mem::size_of::<DROPFILES>(),
        )
    };
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0); // 路径结尾
    wide.push(0); // 列表结尾（双 null）
    let mut out = Vec::with_capacity(head.len() + wide.len() * 2);
    out.extend_from_slice(head);
    for u in wide {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// 把字节拷进一块可移动全局内存，交给剪贴板（SetClipboardData 成功后由系统接管，不能再释放）。
unsafe fn global_from_bytes(data: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GHND, data.len()).ok()?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), p as *mut u8, data.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Annotation, App, Mode, PALETTE, Shape, TEXT_FONT_HEIGHT, TOOLBAR_SLOT_COLOR,
        TOOLBAR_SLOT_COUNT, Tool, ToolbarAction, ToolbarItem, build_dib, color_u32, crop_image,
        draw_annotation_image, draw_line_image, draw_rect_image, gdi_text_size, normalized_rect,
        ocr_region, palette_hit, palette_popup_rect, palette_swatch_rect, prepare_ocr_rgba,
        toolbar_hit, toolbar_item, toolbar_item_rect, toolbar_item_slot, toolbar_origin,
        toolbar_size, unicode_text_bytes,
    };
    use xcap::image::RgbaImage;

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
}

fn show_message(message: &str, error: bool) {
    let text = HSTRING::from(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("rshot"),
            if error {
                MB_ICONERROR
            } else {
                MB_ICONINFORMATION
            },
        );
    }
}

fn main() {
    // release 版没控制台，启动出错会闷声退出；这里把错误弹窗告诉用户
    if let Err(e) = run() {
        let text = HSTRING::from(format!("rshot 启动失败：\n{e}"));
        let caption = HSTRING::from("rshot");
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(caption.as_ptr()),
                MB_ICONERROR,
            );
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    // 最开头声明进程为 per-monitor-v2 DPI aware，赶在 EventLoop 和任何截图之前。
    // 否则高 DPI 屏上 winit 报逻辑尺寸、xcap 截物理尺寸，两者不一致会导致画面斜切。
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let _winrt = WinRtApartment::initialize()?;

    let cfg: Config = confy::load("RShot", None)?;
    let shot_key: HotKey = cfg.hotkey.parse()?;
    let quit_key: HotKey = cfg.quit.parse()?;

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
        .with_icon(make_icon())
        .build()?;

    let app = App {
        shot_id: shot_key.id, // HotKey 是 Copy，register 后仍可取 id
        quit_id: quit_key.id,
        ..Default::default()
    };
    event_loop.run_app(app)?;

    drop(manager); // 显式让 manager 活到这里
    Ok(())
}
