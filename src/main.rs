// release 构建切到 windows 子系统 = 双击不弹黑色控制台窗口。
// debug（cargo run）保留控制台，方便看 println!/panic。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use softbuffer::{Context, Surface};
use std::error::Error;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_HDROP};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Shell::DROPFILES;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindowRect, IsIconic, IsWindowVisible, MB_ICONERROR, MessageBoxW,
};
use windows::core::{BOOL, HSTRING, PCWSTR};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};
use xcap::Monitor;
use xcap::image::{RgbaImage, imageops};

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

#[derive(Default)]
struct App {
    // 两个热键的 id，用来分辨收到的是哪一个
    shot_id: u32,
    quit_id: u32,

    // —— 以下是遮罩窗口的状态，只有正在框选时才有值 ——
    window: Option<Rc<dyn Window>>,
    surface: Option<Surface<Rc<dyn Window>, Rc<dyn Window>>>,
    shot: Vec<u32>,         // 冻屏像素（0RGB，显示用）
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
    strokes: Vec<Vec<(i32, i32)>>,
    drawing: bool,
    pen_active: bool,
    toolbar_hover: Option<usize>,
    toolbar_pressed: Option<usize>,
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
        self.shot = img
            .pixels()
            .map(|px| {
                let [r, g, b, _a] = px.0;
                (r as u32) << 16 | (g as u32) << 8 | b as u32
            })
            .collect();
        self.img = Some(img);
        self.start = None; // 每次开都重置框选
        self.sel = None;
        self.mode = Mode::Selecting;
        self.strokes.clear();
        self.drawing = false;
        self.pen_active = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
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
        window.request_redraw(); // 主动要首帧，否则黑底白窗
        self.window = Some(window);
        self.surface = Some(surface);
    }

    /// 确认截图：手动拖出的框裁框，否则截整屏。截完进剪贴板并收起遮罩。
    fn confirm(&mut self) {
        if let Some(img) = self.output_image() {
            if let Some(w) = &self.window {
                w.set_visible(false); // 先藏，编码耗时挪到看不见后
            }
            image_to_clipboard(&img);
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
        for stroke in &self.strokes {
            for pair in stroke.windows(2) {
                draw_line_image(
                    &mut out,
                    (pair[0].0 - offset.0, pair[0].1 - offset.1),
                    (pair[1].0 - offset.0, pair[1].1 - offset.1),
                    [255, 45, 45, 255],
                    5,
                );
            }
        }
        Some(out)
    }

    fn pin(&mut self) {
        let Some(out) = self.output_image() else {
            return;
        };
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let pos = self
            .sel
            .map(normalized_rect)
            .map(|r| PhysicalPosition::new(self.origin.0 + r.0, self.origin.1 + r.1))
            .unwrap_or(PhysicalPosition::new(self.origin.0, self.origin.1));
        self.shot = out
            .pixels()
            .map(|p| {
                let [r, g, b, _] = p.0;
                (r as u32) << 16 | (g as u32) << 8 | b as u32
            })
            .collect();
        // 给右上角关闭按钮留出最小可点击区域，极小选区也不会丢失控制入口。
        let size = PhysicalSize::new(out.width().max(56), out.height().max(44));
        self.img = Some(out);
        self.sel = None;
        self.strokes.clear();
        self.pen_active = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        self.mode = Mode::Pinned;
        self.pin_drag = None;
        window.set_fullscreen(None);
        window.set_decorations(false);
        window.set_window_level(WindowLevel::AlwaysOnTop);
        let _ = window.request_surface_size(size.into());
        window.set_outer_position(pos.into());
        window.request_redraw();
    }

    fn apply_toolbar_action(&mut self, action: ToolbarAction) {
        match action {
            ToolbarAction::Copy => self.confirm(),
            ToolbarAction::Pen => {
                self.pen_active = !self.pen_active;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            ToolbarAction::Reselect => self.reselect(),
            ToolbarAction::Pin => self.pin(),
            ToolbarAction::Undo => {
                self.strokes.pop();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            ToolbarAction::Close => self.close_overlay(),
        }
    }

    fn reselect(&mut self) {
        // 保留当前冻结画面，只清掉选区和标注，避免重新截屏造成内容变化。
        self.mode = Mode::Selecting;
        self.sel = None;
        self.start = None;
        self.dragged = false;
        self.manual = false;
        self.strokes.clear();
        self.pen_active = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    /// 关掉遮罩窗口，回后台待命（不退程序）。丢掉所有 Rc，窗口即被销毁
    fn close_overlay(&mut self) {
        self.window = None;
        self.surface = None;
        self.img = None;
        self.shot = Vec::new();
        self.start = None;
        self.sel = None;
        self.windows = Vec::new();
        self.dragged = false;
        self.manual = false;
        self.mode = Mode::Selecting;
        self.strokes.clear();
        self.drawing = false;
        self.pen_active = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
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
                        self.strokes.pop();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyB)
                        && self.mode == Mode::Editing
                    {
                        self.pen_active = !self.pen_active;
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
                    if event.physical_key == PhysicalKey::Code(KeyCode::KeyC)
                        && self.mode != Mode::Pinned
                    {
                        self.confirm();
                        return;
                    }
                    if let Key::Named(NamedKey::Escape) = event.logical_key {
                        self.close_overlay();
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
                    let hover_index = hover.map(toolbar_action_index);
                    let hover_changed = hover_index != self.toolbar_hover;
                    self.toolbar_hover = hover_index;
                    if self.drawing {
                        if let Some(stroke) = self.strokes.last_mut() {
                            stroke.push(self.cur);
                        }
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    } else if hover_changed {
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
                    let toolbar_action =
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
                    if toolbar_action.is_some() {
                        match (mb, state) {
                            (Some(MouseButton::Left), ElementState::Pressed) => {
                                self.toolbar_pressed = toolbar_action.map(toolbar_action_index);
                            }
                            (Some(MouseButton::Left), ElementState::Released) => {
                                let pressed = self.toolbar_pressed.take();
                                let action_index = toolbar_action.map(toolbar_action_index);
                                if pressed.is_some() && pressed == action_index {
                                    self.apply_toolbar_action(toolbar_action.unwrap());
                                }
                            }
                            (_, ElementState::Released) => {
                                self.toolbar_pressed = None;
                            }
                            _ => {}
                        }
                        return;
                    }
                    if mb == Some(MouseButton::Left)
                        && state == ElementState::Released
                        && self.toolbar_pressed.take().is_some()
                    {
                        return;
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
                                if self.pen_active && point_in_selection(self.cur, self.sel) {
                                    self.drawing = true;
                                    self.strokes.push(vec![self.cur]);
                                }
                            }
                            ElementState::Released => {
                                if self.drawing {
                                    if let Some(stroke) = self.strokes.last_mut() {
                                        if stroke.len() == 1 {
                                            stroke.push(stroke[0]);
                                        }
                                    }
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
                                    self.pen_active = false;
                                    self.toolbar_hover = None;
                                    self.toolbar_pressed = None;
                                }
                            }
                            // 拖框后进入编辑态：工具栏选择画笔/复制/置顶。
                            else {
                                self.manual = true;
                                self.mode = Mode::Editing;
                                self.pen_active = false;
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
                    for y in 0..ih.min(sh) {
                        let src = &self.shot[y * iw..y * iw + copy_w];
                        buffer[y * sw..y * sw + copy_w].copy_from_slice(src);
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
                for stroke in &self.strokes {
                    for pair in stroke.windows(2) {
                        draw_line_buffer(
                            &mut buffer[..],
                            w.get(),
                            h.get(),
                            pair[0],
                            pair[1],
                            0x00FF2D2D,
                            5,
                        );
                    }
                }
                if self.mode == Mode::Editing {
                    draw_toolbar(
                        &mut buffer[..],
                        w.get(),
                        h.get(),
                        self.sel,
                        self.pen_active,
                        self.toolbar_hover,
                    );
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
const TOOLBAR_WIDTHS: [i32; 6] = [62, 52, 78, 52, 62, 34];
const TOOLBAR_LABELS: [&str; 6] = ["COPY", "PEN", "SELECT", "PIN", "UNDO", "X"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolbarAction {
    Copy,
    Pen,
    Reselect,
    Pin,
    Undo,
    Close,
}

fn toolbar_action(index: usize) -> ToolbarAction {
    match index {
        0 => ToolbarAction::Copy,
        1 => ToolbarAction::Pen,
        2 => ToolbarAction::Reselect,
        3 => ToolbarAction::Pin,
        4 => ToolbarAction::Undo,
        _ => ToolbarAction::Close,
    }
}

fn toolbar_action_index(action: ToolbarAction) -> usize {
    match action {
        ToolbarAction::Copy => 0,
        ToolbarAction::Pen => 1,
        ToolbarAction::Reselect => 2,
        ToolbarAction::Pin => 3,
        ToolbarAction::Undo => 4,
        ToolbarAction::Close => 5,
    }
}

fn toolbar_origin(w: i32, h: i32, sel: Option<((i32, i32), (i32, i32))>) -> (i32, i32) {
    let total_width = TOOLBAR_WIDTHS.iter().sum::<i32>() + TOOLBAR_GAP * 4;
    let (left, top, right, bottom) =
        sel.map(normalized_rect)
            .unwrap_or((w / 2 - 1, h / 2 - 1, w / 2 + 1, h / 2 + 1));
    let max_x = (w - total_width - 8).max(8);
    let x = ((left + right - total_width) / 2).clamp(8, max_x);
    let y = if bottom + TOOLBAR_HEIGHT + 8 <= h {
        bottom + 8
    } else {
        (top - TOOLBAR_HEIGHT - 8).max(8)
    };
    (x, y.clamp(8, (h - TOOLBAR_HEIGHT - 8).max(8)))
}

fn toolbar_button_rect(origin: (i32, i32), index: usize) -> (i32, i32, i32, i32) {
    let mut x = origin.0;
    for width in TOOLBAR_WIDTHS.iter().take(index) {
        x += *width + TOOLBAR_GAP;
    }
    (
        x,
        origin.1,
        x + TOOLBAR_WIDTHS[index],
        origin.1 + TOOLBAR_HEIGHT,
    )
}

fn toolbar_hit(
    p: (i32, i32),
    w: i32,
    h: i32,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<ToolbarAction> {
    let origin = toolbar_origin(w, h, sel);
    (0..TOOLBAR_WIDTHS.len()).find_map(|i| {
        let (x0, y0, x1, y1) = toolbar_button_rect(origin, i);
        (p.0 >= x0 && p.0 < x1 && p.1 >= y0 && p.1 < y1).then(|| toolbar_action(i))
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
    pen_active: bool,
    hover: Option<usize>,
) {
    let origin = toolbar_origin(w as i32, h as i32, sel);
    let total_width = TOOLBAR_WIDTHS.iter().sum::<i32>() + TOOLBAR_GAP * 4;
    draw_fill_rect(
        buf,
        w,
        h,
        (
            origin.0 + 2,
            origin.1 + 3,
            origin.0 + total_width + 2,
            origin.1 + TOOLBAR_HEIGHT + 3,
        ),
        0x00101010,
    );
    draw_fill_rect(
        buf,
        w,
        h,
        (
            origin.0,
            origin.1,
            origin.0 + total_width,
            origin.1 + TOOLBAR_HEIGHT,
        ),
        0x00212631,
    );
    for i in 0..TOOLBAR_WIDTHS.len() {
        let rect = toolbar_button_rect(origin, i);
        let mut color = match toolbar_action(i) {
            ToolbarAction::Copy => 0x002D9B68,
            ToolbarAction::Pen if pen_active => 0x00D88928,
            ToolbarAction::Pen => 0x004B5968,
            ToolbarAction::Reselect => 0x006B5AA8,
            ToolbarAction::Pin => 0x003B78C8,
            ToolbarAction::Undo => 0x00515D6B,
            ToolbarAction::Close => 0x00A83D48,
        };
        if hover == Some(i) {
            color = match toolbar_action(i) {
                ToolbarAction::Close => 0x00D85A65,
                _ => 0x006D91B5,
            };
        }
        draw_fill_rect(buf, w, h, rect, color);
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
        let text_width = (TOOLBAR_LABELS[i].chars().count() as i32 * 12) - 2;
        draw_text(
            buf,
            w,
            h,
            rect.0 + (rect.2 - rect.0 - text_width) / 2,
            rect.1 + 10,
            TOOLBAR_LABELS[i],
            2,
            0x00FFFFFF,
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

/// 把截图放进剪贴板，同时挂两种格式：
/// - CF_DIB 位图：微信/Word/画图 等能贴图的程序直接粘。
/// - CF_HDROP 文件：把图另存成临时 png，终端/资源管理器粘到的是这个文件路径。
fn image_to_clipboard(img: &RgbaImage) {
    let dib = build_dib(img);
    // 存一份临时 png，好让只认文件的地方（命令行）也能粘到路径
    let png_path = std::env::temp_dir().join("rshot.png");
    let hdrop = img.save(&png_path).ok().map(|_| build_hdrop(&png_path));

    unsafe {
        if OpenClipboard(None).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Some(h) = global_from_bytes(&dib) {
            let _ = SetClipboardData(CF_DIB.0 as u32, Some(HANDLE(h.0)));
        }
        if let Some(bytes) = hdrop {
            if let Some(h) = global_from_bytes(&bytes) {
                let _ = SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(h.0)));
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
        App, Mode, ToolbarAction, build_dib, crop_image, draw_line_image, normalized_rect,
        toolbar_action_index, toolbar_hit, toolbar_origin,
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
    fn toolbar_hit_targets_each_button() {
        let sel = Some(((300, 200), (600, 500)));
        let origin = toolbar_origin(1920, 1080, sel);
        let xs = [31, 92, 161, 230, 291, 343];
        let expected = [
            ToolbarAction::Copy,
            ToolbarAction::Pen,
            ToolbarAction::Reselect,
            ToolbarAction::Pin,
            ToolbarAction::Undo,
            ToolbarAction::Close,
        ];
        for (x, action) in xs.into_iter().zip(expected) {
            assert_eq!(
                toolbar_hit((origin.0 + x, origin.1 + 19), 1920, 1080, sel),
                Some(action)
            );
            assert_eq!(
                toolbar_action_index(action),
                expected.iter().position(|a| *a == action).unwrap()
            );
        }
    }

    #[test]
    fn reselect_keeps_frozen_image_but_clears_edit_state() {
        let mut app = App::default();
        app.img = Some(RgbaImage::new(12, 9));
        app.mode = Mode::Editing;
        app.sel = Some(((1, 2), (8, 7)));
        app.strokes.push(vec![(2, 3), (4, 5)]);
        app.pen_active = true;
        app.reselect();
        assert_eq!(app.mode, Mode::Selecting);
        assert!(app.img.is_some());
        assert!(app.sel.is_none());
        assert!(app.strokes.is_empty());
        assert!(!app.pen_active);
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
