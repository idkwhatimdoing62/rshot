use super::{PinFailure, PinFailureStage};
use crate::app::render::{blit_rgba_image, draw_pin_badge};
use crate::app::windows_adapter::{cursor_position, set_window_visible_without_activation};
use softbuffer::{Context, Surface};
use std::num::NonZeroU32;
use std::rc::Rc;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowId, WindowLevel};
use xcap::image::RgbaImage;

pub(super) trait PinWindow {
    fn id(&self) -> WindowId;
    fn set_visible(&self, visible: bool);
    fn request_redraw(&self);
    fn outer_position(&self) -> Option<(i32, i32)>;
    fn set_outer_position(&self, position: (i32, i32));
    fn cursor_position(&self) -> Option<(i32, i32)>;
    fn redraw(&mut self, image: &RgbaImage) -> Result<(), PinFailure>;
    fn close(self: Box<Self>);
}

pub(super) trait PinWindowFactory {
    fn create(
        &self,
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
    ) -> Result<Box<dyn PinWindow>, PinFailure>;
}

pub(super) struct LivePinWindowFactory<'a> {
    event_loop: &'a dyn ActiveEventLoop,
}

impl<'a> LivePinWindowFactory<'a> {
    pub(super) fn new(event_loop: &'a dyn ActiveEventLoop) -> Self {
        Self { event_loop }
    }
}

impl PinWindowFactory for LivePinWindowFactory<'_> {
    fn create(
        &self,
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
    ) -> Result<Box<dyn PinWindow>, PinFailure> {
        LivePinWindow::create(self.event_loop, position, size)
    }
}

pub(super) struct LivePinWindow {
    surface: Surface<Rc<dyn Window>, Rc<dyn Window>>,
    window: Rc<dyn Window>,
}

impl LivePinWindow {
    fn create(
        event_loop: &dyn ActiveEventLoop,
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
    ) -> Result<Box<dyn PinWindow>, PinFailure> {
        let window: Rc<dyn Window> = Rc::from(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_visible(false)
                        .with_decorations(false)
                        .with_window_level(WindowLevel::AlwaysOnTop)
                        .with_surface_size(size),
                )
                .map_err(|error| PinFailure::new(PinFailureStage::CreateWindow, error))?,
        );
        window.set_outer_position(position.into());
        let context = Context::new(window.clone())
            .map_err(|error| PinFailure::new(PinFailureStage::CreateContext, error))?;
        let surface = Surface::new(&context, window.clone())
            .map_err(|error| PinFailure::new(PinFailureStage::CreateSurface, error))?;
        Ok(Box::new(Self { surface, window }))
    }
}

impl PinWindow for LivePinWindow {
    fn id(&self) -> WindowId {
        self.window.id()
    }

    fn set_visible(&self, visible: bool) {
        set_window_visible_without_activation(self.window.as_ref(), visible);
    }

    fn request_redraw(&self) {
        self.window.request_redraw();
    }

    fn outer_position(&self) -> Option<(i32, i32)> {
        self.window.outer_position().ok().map(|p| (p.x, p.y))
    }

    fn set_outer_position(&self, position: (i32, i32)) {
        self.window
            .set_outer_position(PhysicalPosition::new(position.0, position.1).into());
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        cursor_position()
    }

    fn redraw(&mut self, image: &RgbaImage) -> Result<(), PinFailure> {
        let size = self.window.surface_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        self.surface
            .resize(width, height)
            .map_err(|error| PinFailure::new(PinFailureStage::ResizeSurface, error))?;
        let mut buffer = self
            .surface
            .buffer_mut()
            .map_err(|error| PinFailure::new(PinFailureStage::AcquireBuffer, error))?;
        blit_rgba_image(&mut buffer[..], width.get(), height.get(), image);
        draw_pin_badge(&mut buffer[..], width.get(), height.get());
        buffer
            .present()
            .map_err(|error| PinFailure::new(PinFailureStage::Present, error))
    }

    fn close(self: Box<Self>) {
        self.set_visible(false);
        let Self { surface, window } = *self;
        drop(surface);
        drop(window);
    }
}
