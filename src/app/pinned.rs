use super::render::{blit_rgba_image, draw_pin_controls, pin_close_rect};
use super::state::{SessionFailure, SessionFailureStage};
use super::windows_adapter::set_window_visible_without_activation;
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowLevel};
use xcap::image::RgbaImage;

pub(super) const MAX_PINNED_WINDOWS: usize = 8;

pub(super) fn has_pin_capacity(current_count: usize) -> bool {
    current_count < MAX_PINNED_WINDOWS
}

pub(super) fn dragged_window_position(
    cursor_start: (i32, i32),
    window_start: (i32, i32),
    cursor_now: (i32, i32),
) -> (i32, i32) {
    (
        window_start.0 + cursor_now.0 - cursor_start.0,
        window_start.1 + cursor_now.1 - cursor_start.1,
    )
}

/// 一张独立贴图拥有自己的窗口、绘图表面、最终像素和拖动状态。
/// Surface 放在 Window 前，保证默认销毁顺序也先释放绘图表面。
pub(super) struct PinnedWindow {
    surface: Surface<Rc<dyn Window>, Rc<dyn Window>>,
    window: Rc<dyn Window>,
    image: RgbaImage,
    cursor: (i32, i32),
    drag: Option<((i32, i32), (i32, i32))>,
}

pub(super) struct PreparedPinnedWindow {
    surface: Surface<Rc<dyn Window>, Rc<dyn Window>>,
    window: Rc<dyn Window>,
}

impl PreparedPinnedWindow {
    pub(super) fn create(
        event_loop: &dyn ActiveEventLoop,
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
    ) -> Result<Self, SessionFailure> {
        let window: Rc<dyn Window> = Rc::from(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_visible(false)
                        .with_decorations(false)
                        .with_window_level(WindowLevel::AlwaysOnTop)
                        .with_surface_size(size),
                )
                .map_err(|error| SessionFailure::new(SessionFailureStage::CreateWindow, error))?,
        );
        window.set_outer_position(position.into());
        let context = Context::new(window.clone())
            .map_err(|error| SessionFailure::new(SessionFailureStage::CreateContext, error))?;
        let surface = Surface::new(&context, window.clone())
            .map_err(|error| SessionFailure::new(SessionFailureStage::CreateSurface, error))?;
        Ok(Self { surface, window })
    }

    pub(super) fn finish(self, image: RgbaImage) -> PinnedWindow {
        PinnedWindow::new(self.surface, self.window, image)
    }
}

impl PinnedWindow {
    pub(super) fn window_id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    pub(super) fn new(
        surface: Surface<Rc<dyn Window>, Rc<dyn Window>>,
        window: Rc<dyn Window>,
        image: RgbaImage,
    ) -> Self {
        Self {
            surface,
            window,
            image,
            cursor: (0, 0),
            drag: None,
        }
    }

    pub(super) fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub(super) fn set_visible(&self, visible: bool) {
        set_window_visible_without_activation(self.window.as_ref(), visible);
        if visible {
            self.window.request_redraw();
        }
    }

    pub(super) fn set_cursor(&mut self, cursor: (i32, i32)) {
        self.cursor = cursor;
    }

    pub(super) fn close_button_hit(&self) -> bool {
        let size = self.window.surface_size();
        let rect = pin_close_rect(size.width as i32, size.height as i32);
        self.cursor.0 >= rect.0
            && self.cursor.0 < rect.2
            && self.cursor.1 >= rect.1
            && self.cursor.1 < rect.3
    }

    pub(super) fn begin_drag(&mut self, cursor: (i32, i32)) {
        if let Ok(position) = self.window.outer_position() {
            self.drag = Some((cursor, (position.x, position.y)));
        }
    }

    pub(super) fn drag_to(&self, cursor: (i32, i32)) {
        let Some((cursor_start, window_start)) = self.drag else {
            return;
        };
        let (x, y) = dragged_window_position(cursor_start, window_start, cursor);
        self.window
            .set_outer_position(PhysicalPosition::new(x, y).into());
    }

    pub(super) fn end_drag(&mut self) {
        self.drag = None;
    }

    pub(super) fn redraw(&mut self) -> Result<(), SessionFailure> {
        let size = self.window.surface_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        self.surface
            .resize(width, height)
            .map_err(|error| SessionFailure::new(SessionFailureStage::ResizeSurface, error))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|error| SessionFailure::new(SessionFailureStage::AcquireBuffer, error))?;
        blit_rgba_image(&mut buffer[..], width.get(), height.get(), &self.image);
        draw_pin_controls(&mut buffer[..], width.get(), height.get());
        buffer
            .present()
            .map_err(|error| SessionFailure::new(SessionFailureStage::Present, error))
    }

    pub(super) fn close(self) {
        set_window_visible_without_activation(self.window.as_ref(), false);
        let Self {
            surface,
            window,
            image: _,
            cursor: _,
            drag: _,
        } = self;
        drop(surface);
        drop(window);
    }
}
