use super::super::window::LiveCaptureWindow;
use crate::app::geometry::RectI;
use crate::app::state::CaptureFailureStage;
use crate::app::windows_adapter::{cursor_position, visible_window_rects};
use softbuffer::{Context, Surface};
use std::rc::Rc;
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::window::{Window, WindowAttributes};
use xcap::Monitor;
use xcap::image::RgbaImage;

pub(super) fn read_cursor() -> Result<(i32, i32), CaptureFailureStage> {
    cursor_position().ok_or(CaptureFailureStage::ReadCursor)
}

pub(super) fn capture_monitor(cursor: (i32, i32)) -> Result<Monitor, CaptureFailureStage> {
    Monitor::from_point(cursor.0, cursor.1).map_err(|_| CaptureFailureStage::LocateCaptureMonitor)
}

pub(super) fn capture_image(monitor: &Monitor) -> Result<RgbaImage, CaptureFailureStage> {
    monitor
        .capture_image()
        .map_err(|_| CaptureFailureStage::CaptureImage)
}

pub(super) fn visible_windows() -> Vec<RectI> {
    visible_window_rects()
}

pub(super) fn create_overlay(
    event_loop: &dyn ActiveEventLoop,
    monitor: MonitorHandle,
) -> Result<LiveCaptureWindow, CaptureAttemptFailureDetail> {
    let window: Rc<dyn Window> = event_loop
        .create_window(
            WindowAttributes::default()
                .with_fullscreen(Some(winit::monitor::Fullscreen::Borderless(Some(monitor)))),
        )
        .map(Rc::from)
        .map_err(|error| {
            CaptureAttemptFailureDetail::new(CaptureFailureStage::CreateWindow, error)
        })?;
    let context = Context::new(window.clone()).map_err(|error| {
        CaptureAttemptFailureDetail::new(CaptureFailureStage::CreateContext, error)
    })?;
    let surface = Surface::new(&context, window.clone()).map_err(|error| {
        CaptureAttemptFailureDetail::new(CaptureFailureStage::CreateSurface, error)
    })?;
    #[allow(deprecated)]
    window.set_ime_allowed(true);
    window.request_redraw();
    Ok(LiveCaptureWindow::new(window, surface))
}

pub(super) struct CaptureAttemptFailureDetail {
    pub(super) stage: CaptureFailureStage,
    pub(super) detail: String,
}

impl CaptureAttemptFailureDetail {
    fn new(stage: CaptureFailureStage, detail: impl std::fmt::Display) -> Self {
        Self {
            stage,
            detail: detail.to_string(),
        }
    }
}
