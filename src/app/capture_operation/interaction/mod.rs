mod actions;
mod event;
mod state;

use super::{CaptureCommand, CapturePhase};
use crate::app::editor::{Annotation, EditorState};
use crate::app::geometry::RectI;
use crate::app::windows_adapter::gdi_text_size;
use std::time::{Duration, Instant};

pub(super) use state::{EditingState, Interaction, InteractionPhase, SelectingState};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Viewport {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ImeCursorArea {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Default)]
pub(super) struct InteractionOutcome {
    pub(super) command: Option<CaptureCommand>,
    pub(super) redraw: bool,
    pub(super) ime: Option<ImeCursorArea>,
}

pub(super) struct InteractionTick {
    pub(super) outcome: InteractionOutcome,
    pub(super) next_wake: Option<Instant>,
}

pub(super) struct OutputSnapshot<'a> {
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
    pub(super) annotations: &'a [Annotation],
    pub(super) revision: u64,
}

pub(super) struct InteractionFrame<'a> {
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
    pub(super) editor: Option<&'a EditorState>,
}

trait TextMetrics {
    fn measure(&self, text: &str) -> (i32, i32);
}

struct GdiTextMetrics;

impl TextMetrics for GdiTextMetrics {
    fn measure(&self, text: &str) -> (i32, i32) {
        gdi_text_size(text)
    }
}

pub(super) struct InteractionConfig {
    pub(super) cursor: (i32, i32),
    pub(super) origin: (i32, i32),
    pub(super) windows: Vec<RectI>,
    pub(super) image_size: (u32, u32),
}

impl Interaction {
    pub(super) fn new(config: InteractionConfig) -> Self {
        Self::with_metrics(config, Box::new(GdiTextMetrics))
    }

    fn with_metrics(config: InteractionConfig, metrics: Box<dyn TextMetrics>) -> Self {
        state::Interaction::build(config, metrics)
    }

    pub(super) fn frame(&self) -> InteractionFrame<'_> {
        InteractionFrame {
            selection: self.selection(),
            editor: self.editor(),
        }
    }

    pub(super) fn capture_phase(&self) -> CapturePhase {
        match self.phase {
            InteractionPhase::Selecting(_) => CapturePhase::Selecting,
            InteractionPhase::Editing(_) => CapturePhase::Editing,
        }
    }

    pub(super) fn origin(&self) -> (i32, i32) {
        self.origin
    }

    pub(super) fn output_snapshot(&self) -> OutputSnapshot<'_> {
        OutputSnapshot {
            selection: self.selection(),
            annotations: self
                .editor()
                .map(|editor| editor.annotations.as_slice())
                .unwrap_or(&[]),
            revision: self.revision,
        }
    }

    pub(super) fn tick(&mut self, now: Instant) -> InteractionTick {
        let mut outcome = InteractionOutcome::default();
        let next_wake = if self.editor().is_some_and(|editor| editor.text_editing) {
            let last = self.last_blink.get_or_insert(now);
            let redraw = now.duration_since(*last) >= Duration::from_millis(530);
            if redraw {
                *last = now;
            }
            let next = *last + Duration::from_millis(530);
            if redraw {
                if let Some(editor) = self.editor_mut() {
                    editor.cursor_visible = !editor.cursor_visible;
                }
                outcome.redraw = true;
            }
            Some(next)
        } else {
            self.last_blink = None;
            if let Some(editor) = self.editor_mut() {
                editor.cursor_visible = true;
            }
            None
        };
        InteractionTick { outcome, next_wake }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::capture_operation::{CaptureCommand, CapturePhase};
    use crate::app::editor::{PALETTE, Tool, ToolbarItem};
    use std::time::{Duration, Instant};
    use winit::event::WindowEvent;

    struct FixedMetrics;

    impl TextMetrics for FixedMetrics {
        fn measure(&self, text: &str) -> (i32, i32) {
            (text.chars().count() as i32 * 10, 20)
        }
    }

    fn interaction() -> Interaction {
        Interaction::with_metrics(
            InteractionConfig {
                cursor: (20, 20),
                origin: (0, 0),
                windows: Vec::new(),
                image_size: (100, 80),
            },
            Box::new(FixedMetrics),
        )
    }

    fn enter_editing(interaction: &mut Interaction) {
        interaction.set_selection(Some(((10, 10), (70, 60))));
        interaction.finish_selection_gesture(false, None);
    }

    #[test]
    fn new_interaction_is_selecting_without_editor() {
        let interaction = interaction();
        assert_eq!(interaction.capture_phase(), CapturePhase::Selecting);
        assert!(interaction.frame().editor.is_none());
        assert_eq!(interaction.output_snapshot().revision, 0);
    }

    #[test]
    fn editing_starts_with_visible_red_pen() {
        let mut interaction = interaction();
        enter_editing(&mut interaction);
        let editor = interaction.frame().editor.expect("editing frame");
        assert_eq!(editor.tool, Tool::Pen);
        assert_eq!(editor.color, PALETTE[0]);
    }

    #[test]
    fn selection_is_clamped_to_frozen_image() {
        let mut interaction = interaction();
        interaction.set_selection(Some(((-20, 10), (120, 90))));
        assert_eq!(
            interaction.output_snapshot().selection,
            Some(((0, 10), (100, 80)))
        );
    }

    #[test]
    fn reselect_preserves_tool_and_color_but_discards_annotations() {
        let mut interaction = interaction();
        enter_editing(&mut interaction);
        interaction.apply_toolbar_item(ToolbarItem::Tool(Tool::Rect));
        interaction.set_color(4);
        interaction.start_shape((20, 20));
        interaction.reselect();
        enter_editing(&mut interaction);
        let editor = interaction.frame().editor.expect("editing frame");
        assert_eq!(editor.tool, Tool::Rect);
        assert_eq!(editor.color, PALETTE[4]);
        assert!(editor.annotations.is_empty());
    }

    #[test]
    fn transient_tool_change_does_not_change_output_revision() {
        let mut interaction = interaction();
        enter_editing(&mut interaction);
        let before = interaction.output_snapshot().revision;
        interaction.apply_toolbar_item(ToolbarItem::Tool(Tool::Line));
        assert_eq!(interaction.output_snapshot().revision, before);
        interaction.start_shape((5, 5));
        assert!(interaction.output_snapshot().revision > before);
    }

    #[test]
    fn close_event_is_an_exclusive_terminal_outcome() {
        let mut interaction = interaction();
        let outcome = interaction.handle_event(WindowEvent::CloseRequested, Viewport::default());
        assert!(matches!(outcome.command, Some(CaptureCommand::Close)));
        assert!(!outcome.redraw);
        assert!(outcome.ime.is_none());
    }

    #[test]
    fn text_metrics_drive_ime_area_and_tick_requests_one_redraw() {
        let mut interaction = interaction();
        enter_editing(&mut interaction);
        interaction.start_text((5, 7));
        if let Some(editor) = interaction.editor_mut() {
            if let Some(crate::app::editor::Annotation {
                shape: crate::app::editor::Shape::Text(_, text),
                ..
            }) = editor.annotations.last_mut()
            {
                text.push_str("abc");
            }
        }
        interaction.update_ime_area();
        assert_eq!(interaction.ime_requested.expect("IME area").x, 35);
        let start = Instant::now();
        let first = interaction.tick(start);
        assert!(!first.outcome.redraw);
        let second = interaction.tick(start + Duration::from_millis(531));
        assert!(second.outcome.redraw);
        assert!(second.outcome.command.is_none());
    }
}
