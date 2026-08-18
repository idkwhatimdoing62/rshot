use super::window::PinWindow;
use super::{
    PinCollection, PinCollectionState, PinFailure, PinRuntime, PinnedWindow,
    interaction::PinInteraction,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;
use winit::window::WindowId;
use xcap::image::RgbaImage;

struct RecordingRuntime {
    calls: Rc<RefCell<Vec<&'static str>>>,
}

impl PinRuntime for RecordingRuntime {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn flush_compositor(&self) {
        self.calls.borrow_mut().push("flush");
    }
}

struct RecordingWindow {
    calls: Rc<RefCell<Vec<&'static str>>>,
    visible: Rc<Cell<bool>>,
}

impl PinWindow for RecordingWindow {
    fn id(&self) -> WindowId {
        WindowId::from_raw(1)
    }

    fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
        self.calls.borrow_mut().push(if visible {
            "visible:true"
        } else {
            "visible:false"
        });
    }

    fn request_redraw(&self) {
        self.calls.borrow_mut().push("request_redraw");
    }

    fn outer_position(&self) -> Option<(i32, i32)> {
        Some((0, 0))
    }

    fn set_outer_position(&self, _position: (i32, i32)) {}

    fn cursor_position(&self) -> Option<(i32, i32)> {
        Some((0, 0))
    }

    fn redraw(&mut self, _image: &RgbaImage) -> Result<(), PinFailure> {
        Ok(())
    }

    fn close(self: Box<Self>) {
        self.visible.set(false);
    }
}

pub(in crate::app) fn run_pin_coexistence_self_test(
    during_ocr: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let visible = Rc::new(Cell::new(false));
    let collection = PinCollection {
        state: Rc::new(RefCell::new(PinCollectionState {
            windows: HashMap::new(),
            runtime: Box::new(RecordingRuntime {
                calls: calls.clone(),
            }),
            capture_hidden: false,
        })),
    };
    collection.state.borrow_mut().windows.insert(
        WindowId::from_raw(1),
        PinnedWindow {
            window: Box::new(RecordingWindow {
                calls: calls.clone(),
                visible: visible.clone(),
            }),
            image: RgbaImage::new(2, 2),
            interaction: PinInteraction::default(),
        },
    );
    visible.set(true);

    let mut lease = collection
        .hide_for_capture()
        .map_err(|error| error.to_string())?;
    if visible.get() {
        return Err(String::from(
            "pin remained visible while capture pixels were read",
        ));
    }
    lease.complete_capture();
    if !visible.get() {
        return Err(String::from(
            "pin was not restored after capture pixels were read",
        ));
    }

    calls.borrow_mut().clear();
    during_ocr()?;
    if !visible.get() || calls.borrow().contains(&"visible:false") {
        return Err(String::from("OCR changed pin visibility"));
    }
    Ok(())
}
