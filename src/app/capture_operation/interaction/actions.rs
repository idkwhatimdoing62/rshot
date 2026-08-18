use super::{ImeCursorArea, Interaction};
use crate::app::capture_operation::CaptureCommand;
use crate::app::editor::{EditorState, ToolbarAction, ToolbarItem};
use crate::app::geometry::selection_has_area;
use crate::app::output::Shape;
use crate::app::windows_adapter::TEXT_FONT_HEIGHT;

impl Interaction {
    pub(in crate::app::capture_operation) fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    pub(in crate::app::capture_operation) fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(in crate::app::capture_operation) fn set_selection(
        &mut self,
        selection: Option<((i32, i32), (i32, i32))>,
    ) {
        let clamp = |point: (i32, i32)| {
            (
                point.0.clamp(0, self.image_size.0 as i32),
                point.1.clamp(0, self.image_size.1 as i32),
            )
        };
        let selection = selection.map(|(a, b)| (clamp(a), clamp(b)));
        if let super::InteractionPhase::Selecting(state) = &mut self.phase
            && state.selection != selection
        {
            state.selection = selection;
            self.bump_revision();
        }
    }

    pub(in crate::app::capture_operation) fn window_under_cursor(
        &self,
    ) -> Option<((i32, i32), (i32, i32))> {
        let sx = self.cursor.0 + self.origin.0;
        let sy = self.cursor.1 + self.origin.1;
        self.windows.iter().find_map(|rect| {
            (sx >= rect.left && sx < rect.right && sy >= rect.top && sy < rect.bottom).then(|| {
                (
                    (rect.left - self.origin.0 + 1, rect.top - self.origin.1 + 1),
                    (
                        rect.right - self.origin.0 - 1,
                        rect.bottom - self.origin.1 - 1,
                    ),
                )
            })
        })
    }

    pub(in crate::app::capture_operation) fn finish_pointer_selection(&mut self, was_drag: bool) {
        let hovered = self.window_under_cursor();
        self.finish_selection_gesture(was_drag, hovered);
    }

    pub(in crate::app::capture_operation) fn finish_selection_gesture(
        &mut self,
        was_drag: bool,
        hovered: Option<((i32, i32), (i32, i32))>,
    ) {
        if was_drag && !selection_has_area(self.selection()) {
            self.set_selection(hovered);
            self.set_manual(false);
            return;
        }
        let Some(selection) = self.selection() else {
            return;
        };
        let mut editor = EditorState::default();
        editor.tool = self.preferred_tool;
        editor.color = self.preferred_color;
        self.phase = super::InteractionPhase::Editing(super::EditingState { selection, editor });
    }

    pub(in crate::app::capture_operation) fn reselect(&mut self) {
        if let Some((tool, color)) = self.editor().map(|editor| (editor.tool, editor.color)) {
            self.preferred_tool = tool;
            self.preferred_color = color;
        }
        self.phase = super::InteractionPhase::Selecting(super::SelectingState {
            selection: None,
            start: None,
            dragged: false,
            manual: false,
        });
        self.bump_revision();
    }

    pub(in crate::app::capture_operation) fn close_palette(&mut self) {
        if let Some(editor) = self.editor_mut() {
            editor.close_palette();
        }
    }

    pub(in crate::app::capture_operation) fn set_color(&mut self, index: usize) {
        if let Some(editor) = self.editor_mut() {
            let changed_output = editor.text_editing
                && editor.annotations.last().is_some_and(|annotation| {
                    annotation.color != crate::app::editor::PALETTE[index]
                });
            editor.set_color(index);
            self.preferred_color = editor.color;
            if changed_output {
                self.bump_revision();
            }
        }
    }

    pub(in crate::app::capture_operation) fn start_shape(&mut self, point: (i32, i32)) {
        if let Some(editor) = self.editor_mut() {
            editor.start_shape(point);
            self.bump_revision();
        }
    }

    pub(in crate::app::capture_operation) fn update_draft(&mut self, point: (i32, i32)) {
        if let Some(editor) = self.editor_mut() {
            editor.update_draft(point);
            self.bump_revision();
        }
    }

    pub(in crate::app::capture_operation) fn commit_draft(&mut self) {
        if let Some(editor) = self.editor_mut() {
            editor.commit_draft();
            self.bump_revision();
        }
    }

    pub(in crate::app::capture_operation) fn start_text(&mut self, point: (i32, i32)) {
        if let Some(editor) = self.editor_mut() {
            editor.start_text(point);
            self.bump_revision();
            self.last_blink = None;
            self.update_ime_area();
            self.request_redraw();
        }
    }

    pub(in crate::app::capture_operation) fn commit_text(&mut self) {
        if self.editor_mut().is_some_and(EditorState::commit_text) {
            self.bump_revision();
        }
    }

    pub(in crate::app::capture_operation) fn cancel_text(&mut self) {
        if self.editor_mut().is_some_and(EditorState::cancel_text) {
            self.bump_revision();
            self.request_redraw();
        }
    }

    pub(in crate::app::capture_operation) fn update_ime_area(&mut self) {
        let Some(editor) = self.editor() else { return };
        let Some(annotation) = editor.annotations.last() else {
            return;
        };
        let Shape::Text((x, y), text) = &annotation.shape else {
            return;
        };
        let full = format!("{text}{}", editor.ime_preedit);
        let (width, _) = self.metrics.measure(&full);
        self.ime_requested = Some(ImeCursorArea {
            x: x + width,
            y: *y,
            width: 2,
            height: (TEXT_FONT_HEIGHT + 4) as u32,
        });
    }

    pub(in crate::app::capture_operation) fn apply_toolbar_item(
        &mut self,
        item: ToolbarItem,
    ) -> CaptureCommand {
        self.commit_text();
        match item {
            ToolbarItem::Tool(tool) => {
                if let Some(editor) = self.editor_mut() {
                    editor.tool = tool;
                    self.preferred_tool = tool;
                }
                self.request_redraw();
                CaptureCommand::None
            }
            ToolbarItem::Color => {
                if let Some(editor) = self.editor_mut() {
                    editor.palette_open = !editor.palette_open;
                    editor.palette_pressed = None;
                }
                self.request_redraw();
                CaptureCommand::None
            }
            ToolbarItem::Action(ToolbarAction::Copy) => CaptureCommand::Copy,
            ToolbarItem::Action(ToolbarAction::Ocr) => CaptureCommand::Ocr,
            ToolbarItem::Action(ToolbarAction::Pin) => CaptureCommand::Pin,
            ToolbarItem::Action(ToolbarAction::Close) => CaptureCommand::Close,
            ToolbarItem::Action(ToolbarAction::Reselect) => {
                self.reselect();
                self.request_redraw();
                CaptureCommand::None
            }
            ToolbarItem::Action(ToolbarAction::Undo) => {
                if let Some(editor) = self.editor_mut() {
                    if editor.annotations.pop().is_some() {
                        self.bump_revision();
                    }
                }
                self.request_redraw();
                CaptureCommand::None
            }
        }
    }
}
