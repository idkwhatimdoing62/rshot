use super::frame::{CaptureFrame, render_frame};
use crate::app::state::{SessionFailure, SessionFailureStage};
use crate::app::windows_adapter::window_hwnd;
use softbuffer::Surface;
use std::num::NonZeroU32;
use std::rc::Rc;
use windows::Win32::Foundation::HWND;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::{Window, WindowId};

pub(crate) trait CaptureWindow {
    fn id(&self) -> WindowId;
    fn owner_hwnd(&self) -> Option<HWND>;
    fn surface_size(&self) -> PhysicalSize<u32>;
    fn set_visible(&self, visible: bool);
    fn request_redraw(&self);
    fn set_ime_cursor_area(&self, position: PhysicalPosition<i32>, size: PhysicalSize<u32>);
    fn render(&mut self, frame: &CaptureFrame<'_>) -> Result<(), SessionFailure>;
    fn close(&mut self);
}

pub(crate) struct LiveCaptureWindow {
    surface: Option<Surface<Rc<dyn Window>, Rc<dyn Window>>>,
    window: Option<Rc<dyn Window>>,
}

impl LiveCaptureWindow {
    pub(crate) fn new(
        window: Rc<dyn Window>,
        surface: Surface<Rc<dyn Window>, Rc<dyn Window>>,
    ) -> Self {
        Self {
            surface: Some(surface),
            window: Some(window),
        }
    }
}

impl CaptureWindow for LiveCaptureWindow {
    fn id(&self) -> WindowId {
        self.window.as_ref().expect("open capture window").id()
    }

    fn owner_hwnd(&self) -> Option<HWND> {
        self.window
            .as_ref()
            .and_then(|window| window_hwnd(window.as_ref()))
    }

    fn surface_size(&self) -> PhysicalSize<u32> {
        self.window
            .as_ref()
            .expect("open capture window")
            .surface_size()
    }

    fn set_visible(&self, visible: bool) {
        if let Some(window) = &self.window {
            window.set_visible(visible);
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    #[allow(deprecated)]
    fn set_ime_cursor_area(&self, position: PhysicalPosition<i32>, size: PhysicalSize<u32>) {
        if let Some(window) = &self.window {
            window.set_ime_cursor_area(position.into(), size.into());
        }
    }

    fn render(&mut self, frame: &CaptureFrame<'_>) -> Result<(), SessionFailure> {
        let size = self.surface_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        let surface = self.surface.as_mut().ok_or_else(|| {
            SessionFailure::new(
                SessionFailureStage::AccessSurface,
                "活动窗口没有对应的绘图表面",
            )
        })?;
        surface
            .resize(width, height)
            .map_err(|error| SessionFailure::new(SessionFailureStage::ResizeSurface, error))?;
        let mut buffer = surface
            .buffer_mut()
            .map_err(|error| SessionFailure::new(SessionFailureStage::AcquireBuffer, error))?;
        render_frame(&mut buffer, width.get(), height.get(), frame);
        buffer
            .present()
            .map_err(|error| SessionFailure::new(SessionFailureStage::Present, error))
    }

    fn close(&mut self) {
        if let Some(window) = &self.window {
            window.set_visible(false);
        }
        drop(self.surface.take());
        drop(self.window.take());
    }
}

impl Drop for LiveCaptureWindow {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
pub(super) struct TestCaptureWindow;

#[cfg(test)]
impl CaptureWindow for TestCaptureWindow {
    fn id(&self) -> WindowId {
        WindowId::from_raw(1)
    }

    fn owner_hwnd(&self) -> Option<HWND> {
        None
    }

    fn surface_size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(100, 100)
    }

    fn set_visible(&self, _visible: bool) {}
    fn request_redraw(&self) {}
    fn set_ime_cursor_area(&self, _position: PhysicalPosition<i32>, _size: PhysicalSize<u32>) {}
    fn render(&mut self, _frame: &CaptureFrame<'_>) -> Result<(), SessionFailure> {
        Ok(())
    }
    fn close(&mut self) {}
}
