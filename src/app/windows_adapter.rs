use super::geometry::RectI;
use std::error::Error;
use std::ffi::c_void;
use tray_icon::Icon;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmFlush, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_CALCRECT, DT_LEFT, DT_NOPREFIX, DT_TOP, DeleteDC, DeleteObject, DrawTextW,
    FF_DONTCARE, FW_NORMAL, GetDIBits, HGDIOBJ, OPAQUE, OUT_DEFAULT_PRECIS, SelectObject,
    SetBkColor, SetBkMode, SetTextColor,
};
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindowRect, IsIconic, IsWindowVisible, MB_ICONERROR,
    MB_ICONINFORMATION, MessageBoxW, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// 主 UI 线程使用单线程 WinRT apartment；OCR 生命周期覆盖整个事件循环。
pub(super) struct WinRtApartment;

fn window_hwnd(window: &dyn Window) -> Option<HWND> {
    let handle = window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(raw) => Some(HWND(raw.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// 隐藏或恢复贴图时不激活窗口，避免截图结束后从原应用抢走焦点。
pub(super) fn set_window_visible_without_activation(window: &dyn Window, visible: bool) {
    if let Some(hwnd) = window_hwnd(window) {
        unsafe {
            let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    } else {
        window.set_visible(visible);
    }
}

pub(super) fn flush_window_compositor() {
    unsafe {
        let _ = DwmFlush();
    }
}

impl WinRtApartment {
    pub(super) fn initialize() -> windows::core::Result<Self> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        Ok(Self)
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

/// 文字标注的字体高度（物理像素）。
pub(super) const TEXT_FONT_HEIGHT: i32 = 20;

/// 建文字标注用的字体：微软雅黑（覆盖中文），负高度 = 按像素。
pub(super) unsafe fn create_text_font() -> windows::Win32::Graphics::Gdi::HFONT {
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
pub(super) fn gdi_text_size(text: &str) -> (i32, i32) {
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
pub(super) fn gdi_render_text_rgba(text: &str, color: [u8; 4]) -> Option<(i32, i32, Vec<u8>)> {
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

/// 返回当前可见、未最小化的顶层窗口矩形；Win32 句柄和回调不泄漏到应用状态。
pub(super) fn visible_window_rects() -> Vec<RectI> {
    let mut windows = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_cb),
            LPARAM(&mut windows as *mut Vec<RectI> as isize),
        );
    }
    windows
}

unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<RectI>) };
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return BOOL(1);
        }
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
        let mut rect = RECT::default();
        let dwm_ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok();
        if !dwm_ok && GetWindowRect(hwnd, &mut rect).is_err() {
            return BOOL(1);
        }
        if rect.right - rect.left >= 40 && rect.bottom - rect.top >= 40 {
            list.push(RectI {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            });
        }
    }
    BOOL(1)
}

pub(super) fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point).ok()? };
    Some((point.x, point.y))
}

pub(super) fn enable_per_monitor_dpi() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 代码画一个"取景框"图标：透明底 + 四个角标（截图/框选的通用意象）。
pub(super) fn make_icon() -> Result<Icon, Box<dyn Error>> {
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
    Ok(Icon::from_rgba(px, N as u32, N as u32)?)
}

pub(super) fn show_message(message: &str, error: bool) {
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
