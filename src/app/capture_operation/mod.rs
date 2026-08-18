mod attempt;
mod event;
mod frame;
mod interaction;
mod window;

pub(super) use frame::CaptureFrame;

use super::geometry::RectI;
use super::geometry::normalized_rect;
#[cfg(test)]
use super::output::{Annotation, Shape};
use super::output::{OutputDescription, OutputFailureStage, compose};
use super::state::SessionFailure;
use interaction::{Interaction, InteractionConfig, InteractionOutcome};
use std::time::Instant;
use xcap::image::RgbaImage;

pub(super) use attempt::{CaptureAttemptContext, CaptureAttemptFailure};

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CapturePhase {
    Preparing,
    Selecting,
    Editing,
}

pub(super) enum CaptureCommand {
    None,
    Close,
    Copy,
    Ocr,
    Pin,
    Failed(SessionFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaptureAccessError {
    NotReady,
    WindowUnavailable,
    StalePlan,
    Output(OutputFailureStage),
}

impl CaptureAccessError {
    pub(super) const fn output_stage(self) -> Option<OutputFailureStage> {
        match self {
            Self::Output(stage) => Some(stage),
            _ => None,
        }
    }
}

pub(super) struct CopySource<'a> {
    pub(super) owner: windows::Win32::Foundation::HWND,
    pub(super) output: super::output::ScreenshotOutput<'a>,
}

pub(super) struct OcrSource<'a> {
    pub(super) frozen_image: &'a RgbaImage,
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
    pub(super) owner: windows::Win32::Foundation::HWND,
}

pub(super) struct PinPlan {
    revision: u64,
    pub(super) position: winit::dpi::PhysicalPosition<i32>,
    pub(super) size: winit::dpi::PhysicalSize<u32>,
    image: PreparedPinImage,
}

enum PreparedPinImage {
    ReuseFrozen,
    Owned(RgbaImage),
}

pub(super) struct PinCommit {
    image: RgbaImage,
}

pub(super) struct CaptureOperation {
    state: CaptureState,
}

enum CaptureState {
    Preparing,
    Session(Box<CaptureSession>),
}

#[cfg(test)]
pub(super) struct CaptureView<'a> {
    pub(super) frozen_image: &'a RgbaImage,
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
}

pub(super) struct CapturedSession {
    frozen_image: RgbaImage,
    window: Box<dyn window::CaptureWindow>,
    cursor: (i32, i32),
    origin: (i32, i32),
    windows: Vec<RectI>,
}

impl CapturedSession {
    pub(super) fn new(
        frozen_image: RgbaImage,
        window: Box<dyn window::CaptureWindow>,
        cursor: (i32, i32),
        origin: (i32, i32),
        windows: Vec<RectI>,
    ) -> Self {
        Self {
            frozen_image,
            window,
            cursor,
            origin,
            windows,
        }
    }
}

pub(super) struct CaptureSession {
    pub(super) window: Option<Box<dyn window::CaptureWindow>>,
    frozen_image: RgbaImage,
    interaction: Interaction,
}

impl CaptureOperation {
    pub(super) fn start(context: CaptureAttemptContext<'_>) -> Result<Self, CaptureAttemptFailure> {
        let captured = attempt::capture(context)?;
        Ok(Self::begin().attach_capture(captured))
    }

    fn begin() -> Self {
        Self {
            state: CaptureState::Preparing,
        }
    }

    #[cfg(test)]
    pub(super) fn phase(&self) -> CapturePhase {
        match &self.state {
            CaptureState::Preparing => CapturePhase::Preparing,
            CaptureState::Session(session) => session.interaction.capture_phase(),
        }
    }

    pub(super) fn tick(&mut self, now: Instant) -> Option<Instant> {
        let CaptureState::Session(session) = &mut self.state else {
            return None;
        };
        let tick = session.interaction.tick(now);
        session.apply_interaction_outcome(tick.outcome);
        tick.next_wake
    }

    pub(super) fn attach_capture(self, captured: CapturedSession) -> Self {
        debug_assert!(matches!(self.state, CaptureState::Preparing));
        Self {
            state: CaptureState::Session(Box::new(CaptureSession {
                window: Some(captured.window),
                ..CaptureSession::new(
                    captured.frozen_image,
                    captured.cursor,
                    captured.origin,
                    captured.windows,
                )
            })),
        }
    }

    pub(super) fn capture_succeeded_without_window(self, frozen_image: RgbaImage) -> Self {
        self.attach_capture(CapturedSession::new(
            frozen_image,
            Box::new(window::SelfTestCaptureWindow),
            (0, 0),
            (0, 0),
            Vec::new(),
        ))
    }

    pub(super) fn ready_without_window(frozen_image: RgbaImage) -> Self {
        Self::begin().capture_succeeded_without_window(frozen_image)
    }

    #[cfg(test)]
    pub(super) fn test_session(&self) -> &CaptureSession {
        match &self.state {
            CaptureState::Session(session) => session,
            CaptureState::Preparing => panic!("preparing capture has no session state"),
        }
    }

    #[cfg(test)]
    pub(super) fn test_session_mut(&mut self) -> &mut CaptureSession {
        match &mut self.state {
            CaptureState::Session(session) => session,
            CaptureState::Preparing => panic!("preparing capture has no session state"),
        }
    }

    #[cfg(test)]
    pub(super) fn seed_editing_text_for_test(&mut self, text: &str, preedit: &str) {
        let session = self.test_session_mut();
        session.interaction.set_selection(Some(((1, 1), (5, 4))));
        session.interaction.finish_selection_gesture(false, None);
        if let Some(editor) = session.interaction.editor_mut() {
            editor.text_editing = true;
            editor.ime_preedit = preedit.to_owned();
            editor.annotations.push(Annotation {
                shape: Shape::Text((1, 1), text.to_owned()),
                color: super::editor::PALETTE[0],
            });
        }
    }

    #[cfg(test)]
    pub(super) fn enter_editing(&mut self) {
        if let CaptureState::Session(session) = &mut self.state {
            session.interaction.finish_selection_gesture(false, None);
        }
    }

    #[cfg(test)]
    pub(super) fn frozen_image(&self) -> &RgbaImage {
        match &self.state {
            CaptureState::Session(session) => &session.frozen_image,
            CaptureState::Preparing => panic!("preparing capture has no frozen image"),
        }
    }

    #[cfg(test)]
    pub(super) fn selection(&self) -> Option<((i32, i32), (i32, i32))> {
        match &self.state {
            CaptureState::Session(session) => session.interaction.selection(),
            CaptureState::Preparing => None,
        }
    }

    #[cfg(test)]
    pub(super) fn set_selection(&mut self, selection: Option<((i32, i32), (i32, i32))>) {
        if let CaptureState::Session(session) = &mut self.state {
            session.interaction.set_selection(selection);
        }
    }

    #[cfg(test)]
    pub(super) fn finish_selection_gesture(
        &mut self,
        was_drag: bool,
        hovered_window: Option<((i32, i32), (i32, i32))>,
    ) {
        if let CaptureState::Session(session) = &mut self.state {
            session
                .interaction
                .finish_selection_gesture(was_drag, hovered_window);
        }
    }

    #[cfg(test)]
    pub(super) fn reselect(&mut self) {
        if let CaptureState::Session(session) = &mut self.state {
            session.interaction.reselect();
        }
    }

    pub(super) fn close(mut self) {
        let CaptureState::Session(session) = &mut self.state else {
            return;
        };
        if let Some(window) = session.window.as_mut() {
            window.close();
        }
        drop(session.window.take());
    }

    pub(super) fn set_window_visible(&self, visible: bool) {
        if let CaptureState::Session(session) = &self.state
            && let Some(window) = &session.window
        {
            window.set_visible(visible);
        }
    }

    pub(super) fn request_redraw(&self) {
        if let CaptureState::Session(session) = &self.state
            && let Some(window) = &session.window
        {
            window.request_redraw();
        }
    }

    #[cfg(test)]
    pub(super) fn view(&self) -> Option<CaptureView<'_>> {
        let CaptureState::Session(session) = &self.state else {
            return None;
        };
        Some(CaptureView {
            frozen_image: &session.frozen_image,
            selection: session.interaction.selection(),
        })
    }

    pub(super) fn copy_source(&mut self) -> Result<CopySource<'_>, CaptureAccessError> {
        let CaptureState::Session(session) = &mut self.state else {
            return Err(CaptureAccessError::NotReady);
        };
        let snapshot = session.interaction.output_snapshot();
        let owner = session
            .window
            .as_ref()
            .and_then(|window| window.owner_hwnd())
            .ok_or(CaptureAccessError::WindowUnavailable)?;
        let output = compose(OutputDescription {
            frozen_image: &session.frozen_image,
            selection: snapshot.selection,
            annotations: snapshot.annotations,
        })
        .map_err(CaptureAccessError::Output)?;
        Ok(CopySource { owner, output })
    }

    pub(super) fn ocr_source(&mut self) -> Result<OcrSource<'_>, CaptureAccessError> {
        let CaptureState::Session(session) = &mut self.state else {
            return Err(CaptureAccessError::NotReady);
        };
        let snapshot = session.interaction.output_snapshot();
        let owner = session
            .window
            .as_ref()
            .and_then(|window| window.owner_hwnd())
            .ok_or(CaptureAccessError::WindowUnavailable)?;
        Ok(OcrSource {
            frozen_image: &session.frozen_image,
            selection: snapshot.selection,
            owner,
        })
    }

    pub(super) fn prepare_pin(&mut self) -> Result<PinPlan, CaptureAccessError> {
        let CaptureState::Session(session) = &mut self.state else {
            return Err(CaptureAccessError::NotReady);
        };
        let snapshot = session.interaction.output_snapshot();
        let output = compose(OutputDescription {
            frozen_image: &session.frozen_image,
            selection: snapshot.selection,
            annotations: snapshot.annotations,
        })
        .map_err(CaptureAccessError::Output)?;
        let (width, height) = output.dimensions();
        let position = snapshot
            .selection
            .map(normalized_rect)
            .map(|rect| {
                winit::dpi::PhysicalPosition::new(
                    session.interaction.origin().0 + rect.0,
                    session.interaction.origin().1 + rect.1,
                )
            })
            .unwrap_or_else(|| {
                winit::dpi::PhysicalPosition::new(
                    session.interaction.origin().0,
                    session.interaction.origin().1,
                )
            });
        let image = if output.is_borrowed() {
            PreparedPinImage::ReuseFrozen
        } else {
            PreparedPinImage::Owned(output.into_owned())
        };
        Ok(PinPlan {
            revision: snapshot.revision,
            position,
            size: winit::dpi::PhysicalSize::new(width.max(56), height.max(44)),
            image,
        })
    }

    pub(super) fn commit_pin(
        mut self,
        plan: PinPlan,
    ) -> Result<PinCommit, (Self, CaptureAccessError)> {
        let CaptureState::Session(session) = &self.state else {
            return Err((self, CaptureAccessError::NotReady));
        };
        let snapshot = session.interaction.output_snapshot();
        if snapshot.revision != plan.revision {
            return Err((self, CaptureAccessError::StalePlan));
        }
        let image = match plan.image {
            PreparedPinImage::Owned(image) => image,
            PreparedPinImage::ReuseFrozen => match &mut self.state {
                CaptureState::Session(session) => std::mem::take(&mut session.frozen_image),
                CaptureState::Preparing => unreachable!("validated ready session"),
            },
        };
        Ok(PinCommit { image })
    }
}

impl PinCommit {
    pub(super) fn take_image(&mut self) -> RgbaImage {
        std::mem::take(&mut self.image)
    }
}

impl CaptureSession {
    fn new(
        frozen_image: RgbaImage,
        cursor: (i32, i32),
        origin: (i32, i32),
        windows: Vec<RectI>,
    ) -> Self {
        let interaction = Interaction::new(InteractionConfig {
            cursor,
            origin,
            windows: windows.clone(),
            image_size: frozen_image.dimensions(),
        });
        Self {
            window: None,
            frozen_image,
            interaction,
        }
    }

    pub(super) fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn apply_interaction_outcome(&mut self, outcome: InteractionOutcome) -> CaptureCommand {
        if let Some(area) = outcome.ime
            && let Some(window) = &self.window
        {
            window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(area.x, area.y),
                winit::dpi::PhysicalSize::new(area.width, area.height),
            );
        }
        if outcome.redraw {
            self.request_redraw();
        }
        outcome.command.unwrap_or(CaptureCommand::None)
    }

    pub(super) fn render_current(
        &mut self,
        window_id: winit::window::WindowId,
    ) -> Result<(), SessionFailure> {
        let CaptureSession {
            window,
            frozen_image,
            interaction,
            ..
        } = self;
        let Some(window) = window.as_mut() else {
            return Ok(());
        };
        if window.id() != window_id {
            return Ok(());
        }
        let interaction_frame = interaction.frame();
        let frame = CaptureFrame::new(
            frozen_image,
            interaction_frame.selection,
            interaction_frame.editor,
        );
        window.render(&frame)
    }
}

#[cfg(test)]
mod tests {
    use super::interaction::{ImeCursorArea, InteractionOutcome};
    use super::window::CaptureWindow;
    use super::{CaptureOperation, CapturePhase, CapturedSession};
    use crate::app::state::SessionFailure;
    use std::cell::RefCell;
    use std::rc::Rc;
    use windows::Win32::Foundation::HWND;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::window::WindowId;
    use xcap::image::RgbaImage;

    struct RecordingWindow(Rc<RefCell<Vec<&'static str>>>);

    impl CaptureWindow for RecordingWindow {
        fn id(&self) -> WindowId {
            WindowId::from_raw(7)
        }
        fn owner_hwnd(&self) -> Option<HWND> {
            None
        }
        fn surface_size(&self) -> PhysicalSize<u32> {
            PhysicalSize::new(100, 80)
        }
        fn set_visible(&self, _visible: bool) {}
        fn request_redraw(&self) {
            self.0.borrow_mut().push("redraw");
        }
        fn set_ime_cursor_area(&self, _position: PhysicalPosition<i32>, _size: PhysicalSize<u32>) {
            self.0.borrow_mut().push("ime");
        }
        fn render(&mut self, _frame: &super::CaptureFrame<'_>) -> Result<(), SessionFailure> {
            Ok(())
        }
        fn close(&mut self) {}
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
            RgbaImage::from_raw(120, 120, vec![0; 57_600]).expect("valid frozen image"),
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
        assert!(operation.test_session().interaction.is_editing());
    }

    #[test]
    fn reselect_preserves_frozen_image_and_clears_editing_state() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(12, 9, vec![0; 12 * 9 * 4]).expect("valid frozen image"),
        );
        operation.set_selection(Some(((1, 2), (8, 7))));
        operation.enter_editing();
        operation.test_session_mut().interaction.set_manual(true);

        operation.reselect();

        assert_eq!(operation.phase(), CapturePhase::Selecting);
        assert_eq!(operation.frozen_image().dimensions(), (12, 9));
        assert_eq!(operation.selection(), None);
        assert!(!operation.test_session().interaction.manual());
    }

    #[test]
    fn view_borrows_the_current_frame_without_copying_the_frozen_image() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(12, 9, vec![0; 12 * 9 * 4]).expect("valid frozen image"),
        );
        operation.set_selection(Some(((1, 2), (8, 7))));

        let view = operation.view().expect("active session view");

        assert_eq!(view.selection, Some(((1, 2), (8, 7))));
        assert_eq!(
            view.frozen_image.as_ptr(),
            operation.frozen_image().as_ptr()
        );
    }

    #[test]
    fn pin_plan_size_matches_the_selected_output_dimensions() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(1200, 800, vec![0; 1200 * 800 * 4]).expect("valid frozen image"),
        );
        operation.set_selection(Some(((700, 200), (1100, 600))));
        operation.enter_editing();

        let plan = operation.prepare_pin().expect("pin plan");

        assert_eq!(plan.size, PhysicalSize::new(400, 400));
    }

    #[test]
    fn unchanged_full_image_pin_plan_reuses_the_frozen_image() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(120, 80, vec![0; 120 * 80 * 4]).expect("valid frozen image"),
        );

        let plan = operation.prepare_pin().expect("pin plan");

        assert!(matches!(plan.image, super::PreparedPinImage::ReuseFrozen));
    }

    #[test]
    fn raw_close_event_becomes_a_high_level_close_command() {
        let mut operation = CaptureOperation::begin().capture_succeeded_without_window(
            RgbaImage::from_raw(1, 1, vec![0; 4]).expect("valid frozen image"),
        );

        let outcome = operation.test_session_mut().interaction.handle_event(
            winit::event::WindowEvent::CloseRequested,
            super::interaction::Viewport::default(),
        );

        assert!(matches!(
            outcome.command,
            Some(super::CaptureCommand::Close)
        ));
    }

    #[test]
    fn session_applies_ime_before_one_redraw() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut operation = CaptureOperation::begin().attach_capture(CapturedSession::new(
            RgbaImage::new(100, 80),
            Box::new(RecordingWindow(calls.clone())),
            (0, 0),
            (0, 0),
            Vec::new(),
        ));
        let session = operation.test_session_mut();
        session.apply_interaction_outcome(InteractionOutcome {
            command: None,
            redraw: true,
            ime: Some(ImeCursorArea {
                x: 3,
                y: 4,
                width: 2,
                height: 24,
            }),
        });
        assert_eq!(&*calls.borrow(), &["ime", "redraw"]);
    }
}
