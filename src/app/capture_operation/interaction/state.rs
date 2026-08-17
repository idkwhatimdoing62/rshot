use super::{ImeCursorArea, InteractionConfig, TextMetrics, Viewport};
use crate::app::editor::{EditorState, PALETTE, Tool};
use crate::app::geometry::RectI;
use std::time::Instant;
use winit::keyboard::ModifiersState;

pub(in crate::app::capture_operation) struct SelectingState {
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
    pub(super) start: Option<(i32, i32)>,
    pub(super) dragged: bool,
    pub(super) manual: bool,
}

pub(in crate::app::capture_operation) struct EditingState {
    pub(super) selection: ((i32, i32), (i32, i32)),
    pub(super) editor: EditorState,
}

pub(in crate::app::capture_operation) enum InteractionPhase {
    Selecting(SelectingState),
    Editing(EditingState),
}

pub(in crate::app::capture_operation) struct Interaction {
    pub(super) phase: InteractionPhase,
    pub(super) cursor: (i32, i32),
    pub(super) windows: Vec<RectI>,
    pub(super) origin: (i32, i32),
    pub(super) image_size: (u32, u32),
    pub(super) modifiers: ModifiersState,
    pub(super) revision: u64,
    pub(super) last_blink: Option<Instant>,
    pub(super) preferred_tool: Tool,
    pub(super) preferred_color: [u8; 4],
    pub(super) metrics: Box<dyn TextMetrics>,
    pub(super) viewport: Viewport,
    pub(super) redraw_requested: bool,
    pub(super) ime_requested: Option<ImeCursorArea>,
}

impl Interaction {
    pub(super) fn build(config: InteractionConfig, metrics: Box<dyn TextMetrics>) -> Self {
        Self {
            phase: InteractionPhase::Selecting(SelectingState {
                selection: None,
                start: None,
                dragged: false,
                manual: false,
            }),
            cursor: (
                config.cursor.0 - config.origin.0,
                config.cursor.1 - config.origin.1,
            ),
            windows: config.windows,
            origin: config.origin,
            image_size: config.image_size,
            modifiers: ModifiersState::default(),
            revision: 0,
            last_blink: None,
            preferred_tool: Tool::Pen,
            preferred_color: PALETTE[0],
            metrics,
            viewport: Viewport::default(),
            redraw_requested: false,
            ime_requested: None,
        }
    }

    pub(in crate::app::capture_operation) fn selection(&self) -> Option<((i32, i32), (i32, i32))> {
        match &self.phase {
            InteractionPhase::Selecting(state) => state.selection,
            InteractionPhase::Editing(state) => Some(state.selection),
        }
    }

    pub(in crate::app::capture_operation) fn editor(&self) -> Option<&EditorState> {
        match &self.phase {
            InteractionPhase::Selecting(_) => None,
            InteractionPhase::Editing(state) => Some(&state.editor),
        }
    }

    pub(in crate::app::capture_operation) fn editor_mut(&mut self) -> Option<&mut EditorState> {
        match &mut self.phase {
            InteractionPhase::Selecting(_) => None,
            InteractionPhase::Editing(state) => Some(&mut state.editor),
        }
    }

    pub(in crate::app::capture_operation) fn is_editing(&self) -> bool {
        matches!(self.phase, InteractionPhase::Editing(_))
    }

    pub(in crate::app::capture_operation) fn start(&self) -> Option<(i32, i32)> {
        match &self.phase {
            InteractionPhase::Selecting(state) => state.start,
            InteractionPhase::Editing(_) => None,
        }
    }

    pub(in crate::app::capture_operation) fn set_start(&mut self, start: Option<(i32, i32)>) {
        if let InteractionPhase::Selecting(state) = &mut self.phase {
            state.start = start;
        }
    }

    pub(in crate::app::capture_operation) fn dragged(&self) -> bool {
        matches!(&self.phase, InteractionPhase::Selecting(state) if state.dragged)
    }

    pub(in crate::app::capture_operation) fn set_dragged(&mut self, dragged: bool) {
        if let InteractionPhase::Selecting(state) = &mut self.phase {
            state.dragged = dragged;
        }
    }

    pub(in crate::app::capture_operation) fn manual(&self) -> bool {
        matches!(&self.phase, InteractionPhase::Selecting(state) if state.manual)
    }

    pub(in crate::app::capture_operation) fn set_manual(&mut self, manual: bool) {
        if let InteractionPhase::Selecting(state) = &mut self.phase {
            state.manual = manual;
        }
    }
}
