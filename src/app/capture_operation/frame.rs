use crate::app::editor::{EditorState, Tool};
use crate::app::output::{Annotation, render_preview_annotations};
use crate::app::render::*;
use xcap::image::RgbaImage;

pub(crate) struct CaptureFrame<'a> {
    pub(super) frozen_image: &'a RgbaImage,
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
    pub(super) annotations: &'a [Annotation],
    pub(super) editing: bool,
    pub(super) text_editing: bool,
    pub(super) ime_preedit: &'a str,
    pub(super) cursor_visible: bool,
    pub(super) tool: Tool,
    pub(super) color: [u8; 4],
    pub(super) toolbar_hover: Option<usize>,
    pub(super) palette_open: bool,
    pub(super) palette_hover: Option<usize>,
}

impl<'a> CaptureFrame<'a> {
    pub(super) fn new(
        frozen_image: &'a RgbaImage,
        selection: Option<((i32, i32), (i32, i32))>,
        editor: Option<&'a EditorState>,
    ) -> Self {
        let annotations = editor
            .map(|editor| editor.annotations.as_slice())
            .unwrap_or(&[]);
        Self {
            frozen_image,
            selection,
            annotations,
            editing: editor.is_some(),
            text_editing: editor.is_some_and(|editor| editor.text_editing),
            ime_preedit: editor
                .map(|editor| editor.ime_preedit.as_str())
                .unwrap_or(""),
            cursor_visible: editor.is_none_or(|editor| editor.cursor_visible),
            tool: editor.map_or(Tool::Pen, |editor| editor.tool),
            color: editor.map_or([0; 4], |editor| editor.color),
            toolbar_hover: editor.and_then(|editor| editor.toolbar_hover),
            palette_open: editor.is_some_and(|editor| editor.palette_open),
            palette_hover: editor.and_then(|editor| editor.palette_hover),
        }
    }
}

pub(super) fn render_frame(buffer: &mut [u32], width: u32, height: u32, frame: &CaptureFrame<'_>) {
    blit_rgba_image(buffer, width, height, frame.frozen_image);
    if let Some((a, b)) = frame.selection {
        shade_outside(buffer, width, height, a, b);
        draw_rect(buffer, width, height, a.0, a.1, b.0, b.1, 0x00FF0000, 3);
    }
    render_preview_annotations(buffer, width, height, frame.annotations);
    if frame.editing {
        if frame.text_editing
            && let Some(annotation) = frame.annotations.last()
        {
            draw_text_edit_box(
                buffer,
                width,
                height,
                annotation,
                frame.ime_preedit,
                frame.cursor_visible,
            );
        }
        draw_toolbar(
            buffer,
            width,
            height,
            frame.selection,
            frame.tool,
            frame.color,
            frame.toolbar_hover,
            frame.palette_open,
        );
        if frame.palette_open {
            draw_palette_popup(
                buffer,
                width,
                height,
                frame.selection,
                frame.color,
                frame.palette_hover,
            );
        }
    } else {
        draw_select_badge(buffer, width, height);
    }
}
