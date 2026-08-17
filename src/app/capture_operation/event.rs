use super::interaction::Viewport;
use super::{CaptureCommand, CaptureOperation, CaptureState};
use winit::event::WindowEvent;
use winit::window::WindowId;

impl CaptureOperation {
    pub(crate) fn handle_window_event(
        &mut self,
        id: WindowId,
        event: WindowEvent,
    ) -> CaptureCommand {
        let CaptureState::Session(session) = &mut self.state else {
            return CaptureCommand::None;
        };
        let Some(window) = session.window.as_ref() else {
            return CaptureCommand::None;
        };
        if window.id() != id {
            return CaptureCommand::None;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            return match session.render_current(id) {
                Ok(()) => CaptureCommand::None,
                Err(failure) => CaptureCommand::Failed(failure),
            };
        }
        let size = window.surface_size();
        let outcome = session.interaction.handle_event(
            event,
            Viewport {
                width: size.width,
                height: size.height,
            },
        );
        session.apply_interaction_outcome(outcome)
    }
}
