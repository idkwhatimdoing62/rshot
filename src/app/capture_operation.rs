use super::editor::EditorState;
use super::geometry::RectI;
use super::geometry::selection_has_area;
use super::state::CaptureFailureStage;
use softbuffer::Surface;
use std::ops::{Deref, DerefMut};
use std::rc::Rc;
use std::time::Instant;
use winit::keyboard::ModifiersState;
use winit::window::Window;
use xcap::image::RgbaImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CapturePhase {
    Preparing,
    Selecting,
    Editing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaptureEnd {
    CaptureFailed(CaptureFailureStage),
}

pub(super) struct CaptureOperation {
    state: CaptureState,
}

enum CaptureState {
    Preparing,
    Session(Box<CaptureSession>),
}

type WindowSurface = Surface<Rc<dyn Window>, Rc<dyn Window>>;
type RenderPartsMut<'a> = (&'a RgbaImage, &'a EditorState, &'a mut WindowSurface);

pub(super) struct CaptureSession {
    pub(super) surface: Option<Surface<Rc<dyn Window>, Rc<dyn Window>>>,
    pub(super) window: Option<Rc<dyn Window>>,
    pub(super) last_blink: Option<Instant>,
    pub(super) modifiers: ModifiersState,
    state: CaptureSessionState,
}

pub(super) struct CaptureSessionState {
    phase: CapturePhase,
    frozen_image: RgbaImage,
    selection: Option<((i32, i32), (i32, i32))>,
    pub(super) cursor: (i32, i32),
    pub(super) start: Option<(i32, i32)>,
    pub(super) cur: (i32, i32),
    pub(super) windows: Vec<RectI>,
    pub(super) origin: (i32, i32),
    pub(super) dragged: bool,
    pub(super) manual: bool,
    editor: EditorState,
}

impl CaptureOperation {
    pub(super) fn begin() -> Self {
        Self {
            state: CaptureState::Preparing,
        }
    }

    pub(super) fn phase(&self) -> CapturePhase {
        match &self.state {
            CaptureState::Preparing => CapturePhase::Preparing,
            CaptureState::Session(session) => session.state.phase,
        }
    }

    pub(super) fn is_preparing(&self) -> bool {
        self.phase() == CapturePhase::Preparing
    }

    pub(super) fn capture_succeeded(
        self,
        frozen_image: RgbaImage,
        window: Rc<dyn Window>,
        surface: Surface<Rc<dyn Window>, Rc<dyn Window>>,
    ) -> Self {
        debug_assert!(matches!(self.state, CaptureState::Preparing));
        Self {
            state: CaptureState::Session(Box::new(CaptureSession {
                surface: Some(surface),
                window: Some(window),
                last_blink: None,
                modifiers: ModifiersState::default(),
                state: CaptureSessionState::new(frozen_image),
            })),
        }
    }

    #[cfg(test)]
    pub(super) fn capture_succeeded_without_window(self, frozen_image: RgbaImage) -> Self {
        debug_assert!(matches!(self.state, CaptureState::Preparing));
        Self {
            state: CaptureState::Session(Box::new(CaptureSession {
                surface: None,
                window: None,
                last_blink: None,
                modifiers: ModifiersState::default(),
                state: CaptureSessionState::new(frozen_image),
            })),
        }
    }

    pub(super) fn enter_selecting(&mut self) {
        if let CaptureState::Session(session) = &mut self.state {
            session.state.phase = CapturePhase::Selecting;
        }
    }

    pub(super) fn enter_editing(&mut self) {
        if let CaptureState::Session(session) = &mut self.state {
            session.state.phase = CapturePhase::Editing;
        }
    }

    pub(super) fn frozen_image(&self) -> &RgbaImage {
        match &self.state {
            CaptureState::Session(session) => &session.state.frozen_image,
            CaptureState::Preparing => panic!("preparing capture has no frozen image"),
        }
    }

    pub(super) fn selection(&self) -> Option<((i32, i32), (i32, i32))> {
        match &self.state {
            CaptureState::Session(session) => session.state.selection,
            CaptureState::Preparing => None,
        }
    }

    pub(super) fn set_selection(&mut self, selection: Option<((i32, i32), (i32, i32))>) {
        if let CaptureState::Session(session) = &mut self.state {
            session.state.selection = selection;
        }
    }

    pub(super) fn finish_selection_gesture(
        &mut self,
        was_drag: bool,
        hovered_window: Option<((i32, i32), (i32, i32))>,
    ) {
        if was_drag && !selection_has_area(self.selection()) {
            self.set_selection(hovered_window);
            self.enter_selecting();
            self.manual = false;
            return;
        }
        if self.selection().is_some() {
            self.enter_editing();
            self.manual = true;
            self.mode = super::state::Mode::Editing;
            self.toolbar_hover = None;
            self.toolbar_pressed = None;
        }
    }

    pub(super) fn reselect(&mut self) {
        self.set_selection(None);
        self.start = None;
        self.dragged = false;
        self.manual = false;
        EditorState::reset_for_reselect(self);
        self.enter_selecting();
    }

    pub(super) fn into_frozen_image(self) -> Option<RgbaImage> {
        match self.state {
            CaptureState::Session(session) => Some(session.state.frozen_image),
            CaptureState::Preparing => None,
        }
    }

    pub(super) fn capture_failed(self, stage: CaptureFailureStage) -> CaptureEnd {
        CaptureEnd::CaptureFailed(stage)
    }

    pub(super) fn close_window_resources(&mut self) {
        let CaptureState::Session(session) = &mut self.state else {
            return;
        };
        if let Some(window) = session.window.as_ref() {
            window.set_visible(false);
        }
        drop(session.surface.take());
        drop(session.window.take());
    }

    pub(super) fn render_parts_mut(&mut self) -> Option<RenderPartsMut<'_>> {
        let CaptureState::Session(session) = &mut self.state else {
            return None;
        };
        let state = &session.state;
        let surface = session.surface.as_mut()?;
        Some((&state.frozen_image, &state.editor, surface))
    }
}

impl Deref for CaptureOperation {
    type Target = CaptureSession;

    fn deref(&self) -> &Self::Target {
        match &self.state {
            CaptureState::Session(session) => session,
            CaptureState::Preparing => panic!("preparing capture has no editor state"),
        }
    }
}

impl DerefMut for CaptureOperation {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match &mut self.state {
            CaptureState::Session(session) => session,
            CaptureState::Preparing => panic!("preparing capture has no editor state"),
        }
    }
}

impl CaptureSessionState {
    fn new(frozen_image: RgbaImage) -> Self {
        Self {
            phase: CapturePhase::Selecting,
            frozen_image,
            selection: None,
            cursor: (0, 0),
            start: None,
            cur: (0, 0),
            windows: Vec::new(),
            origin: (0, 0),
            dragged: false,
            manual: false,
            editor: EditorState::default(),
        }
    }
}

impl Deref for CaptureSession {
    type Target = CaptureSessionState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for CaptureSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Deref for CaptureSessionState {
    type Target = EditorState;

    fn deref(&self) -> &Self::Target {
        &self.editor
    }
}

impl DerefMut for CaptureSessionState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.editor
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureEnd, CaptureOperation, CapturePhase};
    use crate::app::state::CaptureFailureStage;
    use xcap::image::RgbaImage;

    #[test]
    fn capture_failure_ends_the_preparing_operation_with_a_stable_category() {
        let operation = CaptureOperation::begin();

        assert_eq!(operation.phase(), CapturePhase::Preparing);
        assert_eq!(
            operation.capture_failed(CaptureFailureStage::ReadCursor),
            CaptureEnd::CaptureFailed(CaptureFailureStage::ReadCursor)
        );
    }

    #[test]
    fn successful_capture_enters_selecting_and_owns_the_frozen_image() {
        let operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(2, 1, vec![0; 8]).expect("valid frozen image"),
        );

        assert_eq!(operation.phase(), CapturePhase::Selecting);
        assert_eq!(operation.frozen_image().dimensions(), (2, 1));
    }

    #[test]
    fn zero_area_drag_stays_selecting_and_restores_the_hovered_window() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(2, 1, vec![0; 8]).expect("valid frozen image"),
        );
        operation.set_selection(Some(((10, 20), (80, 20))));

        operation.finish_selection_gesture(true, Some(((11, 11), (99, 99))));

        assert_eq!(operation.phase(), CapturePhase::Selecting);
        assert_eq!(operation.selection(), Some(((11, 11), (99, 99))));
    }

    #[test]
    fn positive_area_drag_enters_editing() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(100, 100, vec![0; 40_000]).expect("valid frozen image"),
        );
        operation.set_selection(Some(((80, 21), (10, 20))));

        operation.finish_selection_gesture(true, None);

        assert_eq!(operation.phase(), CapturePhase::Editing);
        assert!(operation.manual);
    }

    #[test]
    fn reselect_preserves_frozen_image_and_clears_editing_state() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(12, 9, vec![0; 12 * 9 * 4]).expect("valid frozen image"),
        );
        operation.set_selection(Some(((1, 2), (8, 7))));
        operation.enter_editing();
        operation.manual = true;

        operation.reselect();

        assert_eq!(operation.phase(), CapturePhase::Selecting);
        assert_eq!(operation.frozen_image().dimensions(), (12, 9));
        assert_eq!(operation.selection(), None);
        assert!(!operation.manual);
    }
}
