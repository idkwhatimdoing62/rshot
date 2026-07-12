// release 构建切到 windows 子系统 = 双击不弹黑色控制台窗口。
// debug（cargo run）保留控制台，方便看 println!/panic。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use softbuffer::{Context, Surface};
use std::error::Error;
use std::num::NonZeroU32;
use std::ffi::c_void;
use std::rc::Rc;
use std::time::{Duration, Instant};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, POINT, RECT};
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
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};
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

        // 3. 找鼠标那块 winit 屏，建全屏无边框窗口钉上去
        let (cx, cy) = self.cursor;
        let target = event_loop.available_monitors().find(|m| {
            let (Some(pos), Some(mode)) = (m.position(), m.current_video_mode()) else {
                return false;
            };
            let size = mode.size();
            cx >= pos.x && cy >= pos.y && cx < pos.x + size.width as i32 && cy < pos.y + size.height as i32
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
            let _ = EnumWindows(Some(enum_windows_cb), LPARAM(&mut wins as *mut Vec<RECT> as isize));
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
        if let Some(img) = self.img.take() {
            if let Some(w) = &self.window {
                w.set_visible(false); // 先藏，编码耗时挪到看不见后
            }
            // 只有手动拖出的框才裁；悬停锁定的窗口不算，无框=全屏（窗口截图靠单击）
            match self.sel {
                Some((a, b)) if self.manual => crop_to_clipboard(&img, a, b),
                _ => image_to_clipboard(&img),
            }
        }
        self.close_overlay();
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

    fn window_event(&mut self, _event_loop: &dyn ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.close_overlay(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    // ESC：取消，不截图
                    if let Key::Named(NamedKey::Escape) = event.logical_key {
                        self.close_overlay();
                    }
                }
            }
            WindowEvent::PointerMoved { position, .. } => {
                self.cur = (position.x as i32, position.y as i32);
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
                // 右键抬起 = 确认（有手动框裁框，否则全屏）。
                // 必须等抬起：若按下就关遮罩，抬起那半下会漏给下面窗口，触发系统右键菜单
                if mb == Some(MouseButton::Right) && state == ElementState::Released {
                    self.confirm();
                } else if mb == Some(MouseButton::Left) {
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
                                // 单击：直接截当前锁定的框（悬停窗口）→ 剪贴板 → 关
                                if let Some((a, b)) = self.sel {
                                    if let Some(w) = &self.window {
                                        w.set_visible(false);
                                    }
                                    if let Some(img) = self.img.take() {
                                        crop_to_clipboard(&img, a, b);
                                    }
                                    self.close_overlay();
                                }
                            }
                            // 拖框的话：sel 已是手动框，锁住它等右击确认，别被悬停冲掉
                            else {
                                self.manual = true;
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
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
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
                // 再盖 3 像素红框
                if let Some((a, b)) = self.sel {
                    draw_rect(&mut buffer[..], w.get(), h.get(), a.0, a.1, b.0, b.1, 0x00FF0000, 3);
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
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut c_void, 4);
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
fn draw_rect(buf: &mut [u32], w: u32, h: u32, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, t: i32) {
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
        (M, M, M + L, M + T), // 左上 横
        (M, M, M + T, M + L), // 左上 竖
        (N - M - L, M, N - M, M + T), // 右上 横
        (N - M - T, M, N - M, M + L), // 右上 竖
        (M, N - M - T, M + L, N - M), // 左下 横
        (M, N - M - L, M + T, N - M), // 左下 竖
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
fn crop_to_clipboard(img: &RgbaImage, a: (i32, i32), b: (i32, i32)) {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let left = a.0.min(b.0).clamp(0, iw);
    let right = a.0.max(b.0).clamp(0, iw);
    let top = a.1.min(b.1).clamp(0, ih);
    let bottom = a.1.max(b.1).clamp(0, ih);
    let (bw, bh) = ((right - left) as u32, (bottom - top) as u32);
    if bw == 0 || bh == 0 {
        return;
    }
    let cropped = imageops::crop_imm(img, left as u32, top as u32, bw, bh).to_image();
    image_to_clipboard(&cropped);
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
    use super::build_dib;
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
}

fn main() {
    // release 版没控制台，启动出错会闷声退出；这里把错误弹窗告诉用户
    if let Err(e) = run() {
        let text = HSTRING::from(format!("rshot 启动失败：\n{e}"));
        let caption = HSTRING::from("rshot");
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(caption.as_ptr()), MB_ICONERROR);
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
