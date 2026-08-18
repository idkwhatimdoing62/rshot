mod interaction;
mod self_test;
mod window;

pub(super) use self_test::run_pin_coexistence_self_test;

use crate::app::windows_adapter::flush_window_compositor;
use interaction::PinInteraction;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::time::Instant;
use window::{LivePinWindowFactory, PinWindow, PinWindowFactory};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::WindowId;
use xcap::image::RgbaImage;

pub(super) const MAX_PINNED_WINDOWS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PinFailureStage {
    AtCapacity,
    CreateWindow,
    CreateContext,
    CreateSurface,
    ResizeSurface,
    AcquireBuffer,
    Present,
    DuplicateWindowId,
    CaptureAlreadyHidden,
}

impl PinFailureStage {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::AtCapacity => "RSH-PIN-001",
            Self::CreateWindow => "RSH-PIN-002",
            Self::CreateContext => "RSH-PIN-003",
            Self::CreateSurface => "RSH-PIN-004",
            Self::ResizeSurface => "RSH-PIN-005",
            Self::AcquireBuffer => "RSH-PIN-006",
            Self::Present => "RSH-PIN-007",
            Self::DuplicateWindowId => "RSH-PIN-008",
            Self::CaptureAlreadyHidden => "RSH-PIN-009",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AtCapacity => "贴图数量已达上限",
            Self::CreateWindow => "创建贴图窗口",
            Self::CreateContext => "创建贴图图形上下文",
            Self::CreateSurface => "创建贴图绘图表面",
            Self::ResizeSurface => "调整贴图绘图表面尺寸",
            Self::AcquireBuffer => "获取贴图绘图缓冲区",
            Self::Present => "提交贴图绘制结果",
            Self::DuplicateWindowId => "接管贴图窗口",
            Self::CaptureAlreadyHidden => "隐藏捕获期间的贴图",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PinFailure {
    stage: PinFailureStage,
    detail: String,
}

impl PinFailure {
    fn new(stage: PinFailureStage, detail: impl fmt::Display) -> Self {
        Self {
            stage,
            detail: detail.to_string(),
        }
    }

    pub(super) fn stage(&self) -> PinFailureStage {
        self.stage
    }
}

impl fmt::Display for PinFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.detail.is_empty() {
            f.write_str(self.stage.label())
        } else {
            write!(f, "{}失败：{}", self.stage.label(), self.detail)
        }
    }
}

pub(super) enum PinEventOutcome {
    NotOwned(WindowEvent),
    Handled,
    Failed(PinFailure),
}

trait PinRuntime {
    fn now(&self) -> Instant;
    fn flush_compositor(&self);
}

struct LivePinRuntime;

impl PinRuntime for LivePinRuntime {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn flush_compositor(&self) {
        flush_window_compositor();
    }
}

struct PinnedWindow {
    window: Box<dyn PinWindow>,
    image: RgbaImage,
    interaction: PinInteraction,
}

impl PinnedWindow {
    fn close(self) {
        self.window.close();
    }
}

struct PinCollectionState {
    windows: HashMap<WindowId, PinnedWindow>,
    runtime: Box<dyn PinRuntime>,
    capture_hidden: bool,
}

#[derive(Clone)]
pub(super) struct PinCollection {
    state: Rc<RefCell<PinCollectionState>>,
}

pub(in crate::app) struct CaptureVisibilityLease {
    collection: Option<PinCollection>,
}

impl CaptureVisibilityLease {
    pub(in crate::app) fn complete_capture(&mut self) {
        if let Some(collection) = self.collection.take() {
            collection.restore_after_capture();
        }
    }
}

pub(super) struct PreparedPin {
    collection: PinCollection,
    window: Option<Box<dyn PinWindow>>,
}

impl PinCollection {
    pub(super) fn prepare(
        &self,
        event_loop: &dyn ActiveEventLoop,
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
    ) -> Result<PreparedPin, PinFailure> {
        let factory = LivePinWindowFactory::new(event_loop);
        self.prepare_with(&factory, position, size)
    }

    fn prepare_with(
        &self,
        factory: &dyn PinWindowFactory,
        position: PhysicalPosition<i32>,
        size: PhysicalSize<u32>,
    ) -> Result<PreparedPin, PinFailure> {
        if self.state.borrow().windows.len() >= MAX_PINNED_WINDOWS {
            return Err(PinFailure::new(PinFailureStage::AtCapacity, ""));
        }
        self.prepare_created(factory.create(position, size)?)
    }

    fn prepare_created(&self, window: Box<dyn PinWindow>) -> Result<PreparedPin, PinFailure> {
        let state = self.state.borrow();
        if state.windows.len() >= MAX_PINNED_WINDOWS {
            window.close();
            return Err(PinFailure::new(PinFailureStage::AtCapacity, ""));
        }
        if state.windows.contains_key(&window.id()) {
            window.close();
            return Err(PinFailure::new(
                PinFailureStage::DuplicateWindowId,
                "窗口标识已存在",
            ));
        }
        drop(state);
        Ok(PreparedPin {
            collection: self.clone(),
            window: Some(window),
        })
    }

    pub(super) fn handle_window_event(&self, id: WindowId, event: WindowEvent) -> PinEventOutcome {
        let mut state = self.state.borrow_mut();
        if !state.windows.contains_key(&id) {
            return PinEventOutcome::NotOwned(event);
        }
        match event {
            WindowEvent::CloseRequested => {
                Self::close_in(&mut state, id);
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed
                    && matches!(event.logical_key, Key::Named(NamedKey::Escape)) =>
            {
                Self::close_in(&mut state, id);
            }
            WindowEvent::PointerMoved { .. } => {
                let pin = state.windows.get_mut(&id).expect("owned pin");
                if let Some(cursor) = pin.window.cursor_position()
                    && let Some(position) = pin.interaction.drag_to(cursor)
                {
                    pin.window.set_outer_position(position);
                }
            }
            WindowEvent::PointerButton {
                state: button_state,
                button,
                ..
            } => {
                let mouse_button = button.mouse_button();
                if mouse_button == Some(MouseButton::Right)
                    && button_state == ElementState::Released
                {
                    Self::close_in(&mut state, id);
                } else if mouse_button == Some(MouseButton::Left) {
                    let close = {
                        let now = state.runtime.now();
                        let pin = state.windows.get_mut(&id).expect("owned pin");
                        match button_state {
                            ElementState::Pressed => {
                                if let (Some(cursor), Some(position)) =
                                    (pin.window.cursor_position(), pin.window.outer_position())
                                {
                                    pin.interaction.begin_drag(cursor, position);
                                }
                                false
                            }
                            ElementState::Released => pin
                                .interaction
                                .finish_drag(pin.window.cursor_position(), now),
                        }
                    };
                    if close {
                        Self::close_in(&mut state, id);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let redraw = {
                    let pin = state.windows.get_mut(&id).expect("owned pin");
                    pin.window.redraw(&pin.image)
                };
                if let Err(failure) = redraw {
                    Self::close_in(&mut state, id);
                    return PinEventOutcome::Failed(failure);
                }
            }
            _ => {}
        }
        PinEventOutcome::Handled
    }

    pub(super) fn hide_for_capture(&self) -> Result<CaptureVisibilityLease, PinFailure> {
        let mut state = self.state.borrow_mut();
        if state.capture_hidden {
            return Err(PinFailure::new(
                PinFailureStage::CaptureAlreadyHidden,
                "capture already hidden",
            ));
        }
        state.capture_hidden = true;
        for pin in state.windows.values_mut() {
            pin.interaction.cancel();
            pin.window.set_visible(false);
        }
        if !state.windows.is_empty() {
            state.runtime.flush_compositor();
        }
        Ok(CaptureVisibilityLease {
            collection: Some(self.clone()),
        })
    }

    fn restore_after_capture(&self) {
        let mut state = self.state.borrow_mut();
        if !state.capture_hidden {
            return;
        }
        state.capture_hidden = false;
        for pin in state.windows.values() {
            pin.window.set_visible(true);
            pin.window.request_redraw();
        }
        if !state.windows.is_empty() {
            state.runtime.flush_compositor();
        }
    }

    fn close_in(state: &mut PinCollectionState, id: WindowId) {
        if let Some(pin) = state.windows.remove(&id) {
            pin.close();
        }
    }
}

impl Default for PinCollection {
    fn default() -> Self {
        Self {
            state: Rc::new(RefCell::new(PinCollectionState {
                windows: HashMap::new(),
                runtime: Box::new(LivePinRuntime),
                capture_hidden: false,
            })),
        }
    }
}

#[cfg(test)]
impl PinCollection {
    fn test_len(&self) -> usize {
        self.state.borrow().windows.len()
    }

    fn test_is_empty(&self) -> bool {
        self.state.borrow().windows.is_empty()
    }

    fn test_contains(&self, id: WindowId) -> bool {
        self.state.borrow().windows.contains_key(&id)
    }
}

impl PreparedPin {
    pub(super) fn commit(mut self, image: RgbaImage) {
        let window = self.window.take().expect("prepared pin window");
        let id = window.id();
        let mut state = self.collection.state.borrow_mut();
        state.windows.insert(
            id,
            PinnedWindow {
                window,
                image,
                interaction: PinInteraction::default(),
            },
        );
        if !state.capture_hidden {
            let pin = state.windows.get(&id).expect("committed pin");
            pin.window.set_visible(true);
            pin.window.request_redraw();
            state.runtime.flush_compositor();
        }
    }
}

impl Drop for PreparedPin {
    fn drop(&mut self) {
        if let Some(window) = self.window.take() {
            window.close();
        }
    }
}

impl Drop for CaptureVisibilityLease {
    fn drop(&mut self) {
        self.complete_capture();
    }
}

impl Drop for PinCollectionState {
    fn drop(&mut self) {
        for (_, pin) in self.windows.drain() {
            pin.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    struct RecordingRuntime {
        now: Cell<Instant>,
        calls: Rc<RefCell<Vec<String>>>,
    }

    impl PinRuntime for RecordingRuntime {
        fn now(&self) -> Instant {
            self.now.get()
        }

        fn flush_compositor(&self) {
            self.calls.borrow_mut().push(String::from("flush"));
        }
    }

    struct RecordingWindow {
        id: WindowId,
        calls: Rc<RefCell<Vec<String>>>,
        cursor: Rc<Cell<Option<(i32, i32)>>>,
        position: Rc<Cell<(i32, i32)>>,
        redraw_failure: Cell<Option<PinFailureStage>>,
    }

    struct FailingFactory;

    impl PinWindowFactory for FailingFactory {
        fn create(
            &self,
            _position: PhysicalPosition<i32>,
            _size: PhysicalSize<u32>,
        ) -> Result<Box<dyn PinWindow>, PinFailure> {
            Err(PinFailure::new(
                PinFailureStage::CreateWindow,
                "injected failure",
            ))
        }
    }

    impl RecordingWindow {
        fn new(id: usize, calls: Rc<RefCell<Vec<String>>>) -> Self {
            Self {
                id: WindowId::from_raw(id),
                calls,
                cursor: Rc::new(Cell::new(Some((100, 80)))),
                position: Rc::new(Cell::new((400, 300))),
                redraw_failure: Cell::new(None),
            }
        }
    }

    impl PinWindow for RecordingWindow {
        fn id(&self) -> WindowId {
            self.id
        }

        fn set_visible(&self, visible: bool) {
            self.calls.borrow_mut().push(format!("visible:{visible}"));
        }

        fn request_redraw(&self) {
            self.calls.borrow_mut().push(String::from("request_redraw"));
        }

        fn outer_position(&self) -> Option<(i32, i32)> {
            Some(self.position.get())
        }

        fn set_outer_position(&self, position: (i32, i32)) {
            self.position.set(position);
            self.calls
                .borrow_mut()
                .push(format!("move:{},{}", position.0, position.1));
        }

        fn cursor_position(&self) -> Option<(i32, i32)> {
            self.cursor.get()
        }

        fn redraw(&mut self, _image: &RgbaImage) -> Result<(), PinFailure> {
            self.calls.borrow_mut().push(String::from("redraw"));
            match self.redraw_failure.take() {
                Some(stage) => Err(PinFailure::new(stage, "test failure")),
                None => Ok(()),
            }
        }

        fn close(self: Box<Self>) {
            let mut calls = self.calls.borrow_mut();
            calls.push(String::from("visible:false"));
            calls.push(String::from("drop_surface"));
            calls.push(String::from("drop_window"));
        }
    }

    fn collection(calls: Rc<RefCell<Vec<String>>>) -> PinCollection {
        PinCollection {
            state: Rc::new(RefCell::new(PinCollectionState {
                windows: HashMap::new(),
                runtime: Box::new(RecordingRuntime {
                    now: Cell::new(Instant::now()),
                    calls,
                }),
                capture_hidden: false,
            })),
        }
    }

    fn image() -> RgbaImage {
        RgbaImage::from_raw(2, 2, vec![0; 16]).expect("valid pin image")
    }

    fn pointer_button(state: ElementState, button: MouseButton) -> WindowEvent {
        WindowEvent::PointerButton {
            device_id: None,
            state,
            position: PhysicalPosition::new(0.0, 0.0),
            primary: true,
            button: button.into(),
        }
    }

    fn pointer_moved() -> WindowEvent {
        WindowEvent::PointerMoved {
            device_id: None,
            position: PhysicalPosition::new(0.0, 0.0),
            primary: true,
            source: winit::event::PointerSource::Mouse,
        }
    }

    fn escape_pressed() -> WindowEvent {
        WindowEvent::KeyboardInput {
            device_id: None,
            event: winit::event::KeyEvent {
                physical_key: winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Escape),
                logical_key: Key::Named(NamedKey::Escape),
                text: None,
                location: winit::keyboard::KeyLocation::Standard,
                state: ElementState::Pressed,
                repeat: false,
                text_with_all_modifiers: None,
                key_without_modifiers: Key::Named(NamedKey::Escape),
            },
            is_synthetic: false,
        }
    }

    fn commit_window(collection: &PinCollection, id: usize, calls: Rc<RefCell<Vec<String>>>) {
        collection
            .prepare_created(Box::new(RecordingWindow::new(id, calls)))
            .expect("prepare recording pin")
            .commit(image());
    }

    #[test]
    fn first_commit_inserts_then_shows_and_flushes_once() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());

        commit_window(&collection, 1, calls.clone());

        assert_eq!(collection.test_len(), 1);
        assert_eq!(
            calls.borrow().as_slice(),
            ["visible:true", "request_redraw", "flush"]
        );
    }

    #[test]
    fn factory_failure_leaves_the_collection_unchanged() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());

        let failure = match collection.prepare_with(
            &FailingFactory,
            PhysicalPosition::new(10, 20),
            PhysicalSize::new(100, 80),
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("factory failure should be returned"),
        };
        assert_eq!(failure.stage(), PinFailureStage::CreateWindow);
        assert!(collection.test_is_empty());
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn dropping_a_prepared_pin_releases_the_hidden_window_without_inserting() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());

        let prepared = collection
            .prepare_created(Box::new(RecordingWindow::new(1, calls.clone())))
            .expect("prepare recording pin");
        drop(prepared);

        assert!(collection.test_is_empty());
        assert_eq!(
            calls.borrow().as_slice(),
            ["visible:false", "drop_surface", "drop_window"]
        );
    }

    #[test]
    fn capacity_rejects_the_ninth_window_without_changing_the_collection() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        for id in 1..=MAX_PINNED_WINDOWS {
            commit_window(&collection, id, calls.clone());
        }
        calls.borrow_mut().clear();

        let failure =
            match collection.prepare_created(Box::new(RecordingWindow::new(9, calls.clone()))) {
                Err(failure) => failure,
                Ok(_) => panic!("ninth pin should be rejected"),
            };
        assert_eq!(failure.stage(), PinFailureStage::AtCapacity);
        assert_eq!(collection.test_len(), MAX_PINNED_WINDOWS);
        assert_eq!(
            calls.borrow().as_slice(),
            ["visible:false", "drop_surface", "drop_window"]
        );
    }

    #[test]
    fn duplicate_window_id_preserves_the_existing_pin() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls.clone());
        calls.borrow_mut().clear();

        let failure =
            match collection.prepare_created(Box::new(RecordingWindow::new(1, calls.clone()))) {
                Err(failure) => failure,
                Ok(_) => panic!("duplicate id should be rejected"),
            };
        assert_eq!(failure.stage(), PinFailureStage::DuplicateWindowId);
        assert_eq!(collection.test_len(), 1);
    }

    #[test]
    fn hide_and_restore_apply_to_every_pin_and_flush_once_per_transition() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls.clone());
        commit_window(&collection, 2, calls.clone());
        calls.borrow_mut().clear();

        let lease = collection.hide_for_capture().expect("first capture lease");
        drop(lease);

        let calls = calls.borrow();
        assert_eq!(
            calls.iter().filter(|call| *call == "visible:false").count(),
            2
        );
        assert_eq!(
            calls.iter().filter(|call| *call == "visible:true").count(),
            2
        );
        assert_eq!(
            calls
                .iter()
                .filter(|call| *call == "request_redraw")
                .count(),
            2
        );
        assert_eq!(calls.iter().filter(|call| *call == "flush").count(), 2);
    }

    #[test]
    fn completing_pixel_capture_restores_pins_before_the_session_continues() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls.clone());
        calls.borrow_mut().clear();

        let mut lease = collection.hide_for_capture().expect("capture lease");
        lease.complete_capture();
        drop(lease);

        assert_eq!(
            calls.borrow().as_slice(),
            [
                "visible:false",
                "flush",
                "visible:true",
                "request_redraw",
                "flush"
            ]
        );
    }

    #[test]
    fn a_second_capture_visibility_lease_is_rejected() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls);
        let lease = collection.hide_for_capture().expect("first capture lease");

        assert!(collection.hide_for_capture().is_err());

        drop(lease);
        assert!(collection.hide_for_capture().is_ok());
    }

    #[test]
    fn pin_committed_during_capture_is_shown_only_when_the_lease_ends() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        let lease = collection.hide_for_capture().expect("capture lease");
        calls.borrow_mut().clear();

        commit_window(&collection, 1, calls.clone());
        assert!(calls.borrow().is_empty());

        drop(lease);
        assert_eq!(
            calls.borrow().as_slice(),
            ["visible:true", "request_redraw", "flush"]
        );
    }

    #[test]
    fn close_event_is_handled_and_removes_only_the_owned_pin() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls.clone());
        commit_window(&collection, 2, calls.clone());
        calls.borrow_mut().clear();

        let outcome =
            collection.handle_window_event(WindowId::from_raw(1), WindowEvent::CloseRequested);

        assert!(matches!(outcome, PinEventOutcome::Handled));
        assert_eq!(collection.test_len(), 1);
        assert!(collection.test_contains(WindowId::from_raw(2)));
    }

    #[test]
    fn two_left_clicks_close_through_the_raw_event_interface() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls);

        for _ in 0..2 {
            assert!(matches!(
                collection.handle_window_event(
                    WindowId::from_raw(1),
                    pointer_button(ElementState::Pressed, MouseButton::Left),
                ),
                PinEventOutcome::Handled
            ));
            assert!(matches!(
                collection.handle_window_event(
                    WindowId::from_raw(1),
                    pointer_button(ElementState::Released, MouseButton::Left),
                ),
                PinEventOutcome::Handled
            ));
        }

        assert!(collection.test_is_empty());
    }

    #[test]
    fn drag_moves_the_window_and_cancels_the_click_sequence() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        let window = RecordingWindow::new(1, calls.clone());
        let cursor = window.cursor.clone();
        let position = window.position.clone();
        collection
            .prepare_created(Box::new(window))
            .expect("prepare recording pin")
            .commit(image());
        calls.borrow_mut().clear();

        collection.handle_window_event(
            WindowId::from_raw(1),
            pointer_button(ElementState::Pressed, MouseButton::Left),
        );
        cursor.set(Some((125, 110)));
        collection.handle_window_event(WindowId::from_raw(1), pointer_moved());
        collection.handle_window_event(
            WindowId::from_raw(1),
            pointer_button(ElementState::Released, MouseButton::Left),
        );

        assert_eq!(position.get(), (425, 330));
        assert_eq!(collection.test_len(), 1);
    }

    #[test]
    fn right_release_closes_the_owned_pin() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls);

        let outcome = collection.handle_window_event(
            WindowId::from_raw(1),
            pointer_button(ElementState::Released, MouseButton::Right),
        );

        assert!(matches!(outcome, PinEventOutcome::Handled));
        assert!(collection.test_is_empty());
    }

    #[test]
    fn escape_closes_the_owned_pin() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls);

        let outcome = collection.handle_window_event(WindowId::from_raw(1), escape_pressed());

        assert!(matches!(outcome, PinEventOutcome::Handled));
        assert!(collection.test_is_empty());
    }

    #[test]
    fn unknown_window_returns_the_original_event() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls);

        let outcome =
            collection.handle_window_event(WindowId::from_raw(99), WindowEvent::CloseRequested);

        assert!(matches!(
            outcome,
            PinEventOutcome::NotOwned(WindowEvent::CloseRequested)
        ));
    }

    #[test]
    fn redraw_failure_removes_the_failed_pin_and_returns_one_failure() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        let window = RecordingWindow::new(1, calls.clone());
        window
            .redraw_failure
            .set(Some(PinFailureStage::AcquireBuffer));
        collection
            .prepare_created(Box::new(window))
            .expect("prepare recording pin")
            .commit(image());
        calls.borrow_mut().clear();

        let outcome =
            collection.handle_window_event(WindowId::from_raw(1), WindowEvent::RedrawRequested);

        assert!(matches!(
            outcome,
            PinEventOutcome::Failed(PinFailure {
                stage: PinFailureStage::AcquireBuffer,
                ..
            })
        ));
        assert!(collection.test_is_empty());
    }

    #[test]
    fn dropping_the_collection_releases_surface_before_window() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let collection = collection(calls.clone());
        commit_window(&collection, 1, calls.clone());
        calls.borrow_mut().clear();

        drop(collection);

        assert_eq!(
            calls.borrow().as_slice(),
            ["visible:false", "drop_surface", "drop_window"]
        );
    }
}
