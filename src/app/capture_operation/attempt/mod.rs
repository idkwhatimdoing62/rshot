mod platform;
mod target;

use super::CapturedSession;
use crate::app::pinned::PinCollection;
use crate::app::state::CaptureFailureStage;
use platform::*;
use target::match_overlay_monitor;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) struct CaptureAttemptFailure {
    stage: CaptureFailureStage,
    detail: String,
}

impl CaptureAttemptFailure {
    fn at(stage: CaptureFailureStage) -> Self {
        Self {
            stage,
            detail: String::new(),
        }
    }

    pub(in crate::app) fn stage(&self) -> CaptureFailureStage {
        self.stage
    }

    pub(in crate::app) fn detail(&self) -> &str {
        &self.detail
    }
}

pub(in crate::app) struct CaptureAttemptContext<'a> {
    pub(in crate::app) event_loop: &'a dyn winit::event_loop::ActiveEventLoop,
    pub(in crate::app) pins: &'a PinCollection,
}

pub(super) fn capture(
    context: CaptureAttemptContext<'_>,
) -> Result<CapturedSession, CaptureAttemptFailure> {
    let cursor = read_cursor().map_err(CaptureAttemptFailure::at)?;
    let monitor = capture_monitor(cursor).map_err(CaptureAttemptFailure::at)?;
    let target = match_overlay_monitor(context.event_loop, cursor)
        .ok_or_else(|| CaptureAttemptFailure::at(CaptureFailureStage::MatchOverlayMonitor))?;
    let mut visibility =
        context
            .pins
            .hide_for_capture()
            .map_err(|failure| CaptureAttemptFailure {
                stage: CaptureFailureStage::HidePins,
                detail: failure.to_string(),
            })?;
    let frozen_image = capture_image(&monitor).map_err(CaptureAttemptFailure::at)?;
    visibility.complete_capture();
    let windows = visible_windows();
    let window = create_overlay(context.event_loop, target.overlay_monitor).map_err(|failure| {
        CaptureAttemptFailure {
            stage: failure.stage,
            detail: failure.detail,
        }
    })?;
    Ok(CapturedSession::new(
        frozen_image,
        Box::new(window),
        cursor,
        target.origin,
        windows,
    ))
}
