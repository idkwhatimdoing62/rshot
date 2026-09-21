use super::geometry::normalized_rect;
use super::output::{Annotation, Shape};

/// 当前选中的标注工具（编辑模式下左键拖拽用哪个图元）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Tool {
    #[default]
    Pen,
    Line,
    Arrow,
    Rect,
    Mosaic,
    Text,
    Select,
}

/// 一条标注的形状：自由画笔（一串点）/ 直线（两点）/ 矩形（两对角点）/ 文字（左上角锚点 + 内容）。
/// 预设调色板：PEN 默认红放在第一位。
pub(super) const PALETTE: [[u8; 4]; 8] = [
    [255, 45, 45, 255],   // 红
    [245, 102, 0, 255],   // 橙
    [255, 200, 0, 255],   // 黄
    [0, 166, 90, 255],    // 绿
    [59, 120, 200, 255],  // 蓝
    [107, 90, 168, 255],  // 紫
    [255, 255, 255, 255], // 白
    [0, 0, 0, 255],       // 黑
];

/// 与窗口、Surface 和系统 API 无关的编辑会话状态。
pub(super) struct EditorState {
    pub(super) annotations: Vec<Annotation>,
    pub(super) tool: Tool,
    pub(super) color: [u8; 4],
    pub(super) drawing: bool,
    pub(super) toolbar_hover: Option<usize>,
    pub(super) toolbar_pressed: Option<usize>,
    pub(super) palette_open: bool,
    pub(super) palette_hover: Option<usize>,
    pub(super) palette_pressed: Option<usize>,
    pub(super) text_editing: bool,
    pub(super) ime_preedit: String,
    pub(super) cursor_visible: bool,
    pub(super) caret_byte: usize,
    pub(super) selected: Option<usize>,
    text_edit_index: Option<usize>,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    pending_before: Option<Vec<Annotation>>,
    transform: Option<TransformGesture>,
}

#[derive(Clone)]
struct HistoryEntry {
    before: Vec<Annotation>,
    after: Vec<Annotation>,
}

#[derive(Clone)]
struct TransformGesture {
    index: usize,
    start: (i32, i32),
    original: Annotation,
    kind: TransformKind,
}

#[derive(Clone, Copy)]
enum TransformKind {
    Body,
    Start,
    End,
    Corner(RectCorner),
}

#[derive(Clone, Copy)]
enum RectCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

const HISTORY_LIMIT: usize = 100;

impl Default for EditorState {
    fn default() -> Self {
        Self {
            annotations: Vec::new(),
            tool: Tool::Pen,
            color: PALETTE[0],
            drawing: false,
            toolbar_hover: None,
            toolbar_pressed: None,
            palette_open: false,
            palette_hover: None,
            palette_pressed: None,
            text_editing: false,
            ime_preedit: String::new(),
            cursor_visible: true,
            caret_byte: 0,
            selected: None,
            text_edit_index: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending_before: None,
            transform: None,
        }
    }
}

impl EditorState {
    pub(super) fn with_preferences(tool: Tool, color: [u8; 4]) -> Self {
        Self {
            tool,
            color,
            ..Self::default()
        }
    }

    fn begin_change(&mut self) {
        if self.pending_before.is_none() {
            self.pending_before = Some(self.annotations.clone());
        }
    }

    fn finish_change(&mut self) -> bool {
        let Some(before) = self.pending_before.take() else {
            return false;
        };
        if before == self.annotations {
            return false;
        }
        if self.undo_stack.len() == HISTORY_LIMIT {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(HistoryEntry {
            before,
            after: self.annotations.clone(),
        });
        self.redo_stack.clear();
        true
    }

    pub(super) fn undo(&mut self) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        self.pending_before = None;
        self.transform = None;
        self.selected = None;
        self.annotations.clone_from(&entry.before);
        self.redo_stack.push(entry);
        true
    }

    pub(super) fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        self.pending_before = None;
        self.transform = None;
        self.selected = None;
        self.annotations.clone_from(&entry.after);
        self.undo_stack.push(entry);
        true
    }

    pub(super) fn select_at(
        &mut self,
        point: (i32, i32),
        measure_text: impl Fn(&str) -> (i32, i32),
    ) -> Option<usize> {
        let ordinary = self
            .annotations
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, annotation)| !matches!(annotation.shape, Shape::Mosaic(..)));
        let mosaics = self
            .annotations
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, annotation)| matches!(annotation.shape, Shape::Mosaic(..)));
        self.selected = ordinary.chain(mosaics).find_map(|(index, annotation)| {
            annotation_hit(annotation, point, &measure_text).then_some(index)
        });
        self.selected
    }

    pub(super) fn begin_transform_at(&mut self, point: (i32, i32)) -> bool {
        let Some(index) = self.selected else {
            return false;
        };
        let Some(original) = self.annotations.get(index).cloned() else {
            return false;
        };
        let kind = transform_handle(&original.shape, point).unwrap_or(TransformKind::Body);
        self.begin_change();
        self.transform = Some(TransformGesture {
            index,
            start: point,
            original,
            kind,
        });
        true
    }

    pub(super) fn update_transform(&mut self, point: (i32, i32)) -> bool {
        let Some(gesture) = &self.transform else {
            return false;
        };
        let Some(annotation) = self.annotations.get_mut(gesture.index) else {
            return false;
        };
        *annotation = gesture.original.clone();
        match gesture.kind {
            TransformKind::Body => translate_shape(
                &mut annotation.shape,
                (point.0 - gesture.start.0, point.1 - gesture.start.1),
            ),
            TransformKind::Start => match &mut annotation.shape {
                Shape::Line(start, _) | Shape::Arrow(start, _) => *start = point,
                _ => {}
            },
            TransformKind::End => match &mut annotation.shape {
                Shape::Line(_, end) | Shape::Arrow(_, end) => *end = point,
                _ => {}
            },
            TransformKind::Corner(corner) => match &mut annotation.shape {
                Shape::Rect(start, end) | Shape::Mosaic(start, end) => {
                    let (left, top, right, bottom) = normalized_rect((*start, *end));
                    let opposite = match corner {
                        RectCorner::TopLeft => (right, bottom),
                        RectCorner::TopRight => (left, bottom),
                        RectCorner::BottomLeft => (right, top),
                        RectCorner::BottomRight => (left, top),
                    };
                    let normalized = normalized_rect((point, opposite));
                    *start = (normalized.0, normalized.1);
                    *end = (normalized.2, normalized.3);
                }
                _ => {}
            },
        }
        true
    }

    pub(super) fn commit_transform(&mut self) -> bool {
        if self.transform.take().is_none() {
            return false;
        }
        self.finish_change()
    }

    pub(super) fn close_palette(&mut self) {
        self.palette_open = false;
        self.palette_hover = None;
        self.palette_pressed = None;
    }

    pub(super) fn set_color(&mut self, index: usize) -> bool {
        let color = PALETTE[index];
        self.color = color;
        if self.text_editing {
            if let Some(annotation) = self
                .text_edit_index
                .and_then(|index| self.annotations.get_mut(index))
                && annotation.color != color
            {
                annotation.color = color;
                return true;
            }
        } else if let Some(index) = self.selected
            && self
                .annotations
                .get(index)
                .is_some_and(|annotation| annotation.color != color)
        {
            self.begin_change();
            self.annotations[index].color = color;
            return self.finish_change();
        }
        false
    }

    pub(super) fn delete_selected(&mut self) -> bool {
        let Some(index) = self.selected.take() else {
            return false;
        };
        if index >= self.annotations.len() {
            return false;
        }
        self.begin_change();
        self.annotations.remove(index);
        self.finish_change()
    }

    pub(super) fn start_shape(&mut self, point: (i32, i32)) {
        let shape = match self.tool {
            Tool::Pen => Shape::Pen(vec![point]),
            Tool::Line => Shape::Line(point, point),
            Tool::Arrow => Shape::Arrow(point, point),
            Tool::Rect => Shape::Rect(point, point),
            Tool::Mosaic => Shape::Mosaic(point, point),
            Tool::Text | Tool::Select => return,
        };
        self.selected = None;
        self.begin_change();
        self.annotations.push(Annotation {
            shape,
            color: self.color,
        });
    }

    pub(super) fn update_draft(&mut self, point: (i32, i32)) {
        let Some(annotation) = self.annotations.last_mut() else {
            return;
        };
        match &mut annotation.shape {
            Shape::Pen(points) => points.push(point),
            Shape::Line(_, end)
            | Shape::Arrow(_, end)
            | Shape::Rect(_, end)
            | Shape::Mosaic(_, end) => *end = point,
            Shape::Text(..) => {}
        }
    }

    pub(super) fn commit_draft(&mut self) {
        let Some(annotation) = self.annotations.last() else {
            return;
        };
        let (drop, dot) = match &annotation.shape {
            Shape::Pen(points) => (false, points.len() == 1),
            Shape::Line(start, end) | Shape::Arrow(start, end) | Shape::Rect(start, end) => {
                (start == end, false)
            }
            Shape::Mosaic(start, end) => (start.0 == end.0 || start.1 == end.1, false),
            Shape::Text(..) => (false, false),
        };
        if drop {
            self.annotations.pop();
        } else if dot
            && let Some(Shape::Pen(points)) = self.annotations.last_mut().map(|a| &mut a.shape)
        {
            points.push(points[0]);
        }
        self.finish_change();
    }

    pub(super) fn start_text(&mut self, point: (i32, i32)) {
        self.commit_text();
        self.begin_change();
        self.annotations.push(Annotation {
            shape: Shape::Text(point, String::new()),
            color: self.color,
        });
        self.text_edit_index = Some(self.annotations.len() - 1);
        self.selected = None;
        self.text_editing = true;
        self.ime_preedit.clear();
        self.cursor_visible = true;
        self.caret_byte = 0;
    }

    pub(super) fn insert_text(&mut self, value: &str) -> bool {
        let caret = self.caret_byte;
        let Some(Shape::Text(_, text)) = self
            .text_edit_index
            .and_then(|index| self.annotations.get_mut(index))
            .map(|a| &mut a.shape)
        else {
            return false;
        };
        text.insert_str(caret, value);
        self.caret_byte += value.len();
        true
    }

    pub(super) fn backspace(&mut self) -> bool {
        if self.caret_byte == 0 {
            return false;
        }
        let Some(Shape::Text(_, text)) = self
            .text_edit_index
            .and_then(|index| self.annotations.get_mut(index))
            .map(|a| &mut a.shape)
        else {
            return false;
        };
        let previous = text[..self.caret_byte]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        text.drain(previous..self.caret_byte);
        self.caret_byte = previous;
        true
    }

    pub(super) fn move_caret_left(&mut self) {
        if let Some(Shape::Text(_, text)) = self
            .text_edit_index
            .and_then(|index| self.annotations.get(index))
            .map(|a| &a.shape)
        {
            self.caret_byte = text[..self.caret_byte]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub(super) fn move_caret_right(&mut self) {
        if let Some(Shape::Text(_, text)) = self
            .text_edit_index
            .and_then(|index| self.annotations.get(index))
            .map(|a| &a.shape)
            && self.caret_byte < text.len()
        {
            self.caret_byte += text[self.caret_byte..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(0);
        }
    }

    pub(super) fn remove_before_caret_if_matches(&mut self, value: &str) -> bool {
        let count = value.chars().count();
        let Some(Shape::Text(_, text)) = self
            .text_edit_index
            .and_then(|index| self.annotations.get_mut(index))
            .map(|a| &mut a.shape)
        else {
            return false;
        };
        let start = text[..self.caret_byte]
            .char_indices()
            .rev()
            .nth(count.saturating_sub(1))
            .map_or(self.caret_byte, |(index, _)| index);
        if &text[start..self.caret_byte] != value {
            return false;
        }
        text.drain(start..self.caret_byte);
        self.caret_byte = start;
        true
    }

    pub(super) fn commit_text(&mut self) -> bool {
        if !self.text_editing {
            return false;
        }
        self.text_editing = false;
        self.ime_preedit.clear();
        self.caret_byte = 0;
        if let Some(index) = self.text_edit_index.take()
            && self.annotations.get(index).is_some_and(
                |annotation| matches!(&annotation.shape, Shape::Text(_, text) if text.is_empty()),
            )
        {
            self.annotations.remove(index);
        }
        self.finish_change();
        true
    }

    pub(super) fn cancel_text(&mut self) -> bool {
        if !self.text_editing {
            return false;
        }
        self.text_editing = false;
        self.ime_preedit.clear();
        self.caret_byte = 0;
        self.text_edit_index = None;
        if let Some(before) = self.pending_before.take() {
            self.annotations = before;
        }
        true
    }

    pub(super) fn reopen_selected_text(&mut self) -> bool {
        let Some(index) = self.selected else {
            return false;
        };
        let Some(Shape::Text(_, text)) = self.annotations.get(index).map(|a| &a.shape) else {
            return false;
        };
        let caret = text.len();
        self.begin_change();
        self.text_edit_index = Some(index);
        self.text_editing = true;
        self.ime_preedit.clear();
        self.cursor_visible = true;
        self.caret_byte = caret;
        true
    }

    pub(super) fn text_annotation(&self) -> Option<&Annotation> {
        self.text_edit_index
            .and_then(|index| self.annotations.get(index))
    }
}

fn translate_point(point: &mut (i32, i32), delta: (i32, i32)) {
    point.0 += delta.0;
    point.1 += delta.1;
}

fn translate_shape(shape: &mut Shape, delta: (i32, i32)) {
    match shape {
        Shape::Pen(points) => {
            for point in points {
                translate_point(point, delta);
            }
        }
        Shape::Line(start, end)
        | Shape::Arrow(start, end)
        | Shape::Rect(start, end)
        | Shape::Mosaic(start, end) => {
            translate_point(start, delta);
            translate_point(end, delta);
        }
        Shape::Text(point, _) => translate_point(point, delta),
    }
}

fn point_near_segment(point: (i32, i32), start: (i32, i32), end: (i32, i32)) -> bool {
    const TOLERANCE: f64 = 6.0;
    let (px, py) = (f64::from(point.0), f64::from(point.1));
    let (ax, ay) = (f64::from(start.0), f64::from(start.1));
    let (bx, by) = (f64::from(end.0), f64::from(end.1));
    let (dx, dy) = (bx - ax, by - ay);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let (nearest_x, nearest_y) = (ax + t * dx, ay + t * dy);
    (px - nearest_x).hypot(py - nearest_y) <= TOLERANCE
}

fn point_near_handle(point: (i32, i32), handle: (i32, i32)) -> bool {
    const HANDLE_TOLERANCE: i32 = 7;
    (point.0 - handle.0).abs() <= HANDLE_TOLERANCE && (point.1 - handle.1).abs() <= HANDLE_TOLERANCE
}

fn transform_handle(shape: &Shape, point: (i32, i32)) -> Option<TransformKind> {
    match shape {
        Shape::Line(start, end) | Shape::Arrow(start, end) => {
            if point_near_handle(point, *start) {
                Some(TransformKind::Start)
            } else if point_near_handle(point, *end) {
                Some(TransformKind::End)
            } else {
                None
            }
        }
        Shape::Rect(start, end) | Shape::Mosaic(start, end) => {
            let (left, top, right, bottom) = normalized_rect((*start, *end));
            [
                ((left, top), RectCorner::TopLeft),
                ((right, top), RectCorner::TopRight),
                ((left, bottom), RectCorner::BottomLeft),
                ((right, bottom), RectCorner::BottomRight),
            ]
            .into_iter()
            .find_map(|(handle, corner)| {
                point_near_handle(point, handle).then_some(TransformKind::Corner(corner))
            })
        }
        Shape::Pen(..) | Shape::Text(..) => None,
    }
}

fn annotation_hit(
    annotation: &Annotation,
    point: (i32, i32),
    measure_text: &impl Fn(&str) -> (i32, i32),
) -> bool {
    match &annotation.shape {
        Shape::Pen(points) => points
            .windows(2)
            .any(|pair| point_near_segment(point, pair[0], pair[1])),
        Shape::Line(start, end) | Shape::Arrow(start, end) => {
            point_near_segment(point, *start, *end)
        }
        Shape::Rect(start, end) | Shape::Mosaic(start, end) => {
            let (left, top, right, bottom) = normalized_rect((*start, *end));
            point.0 >= left && point.0 <= right && point.1 >= top && point.1 <= bottom
        }
        Shape::Text(origin, text) => {
            let (width, height) = measure_text(text);
            point.0 >= origin.0
                && point.0 <= origin.0 + width.max(4)
                && point.1 >= origin.1
                && point.1 <= origin.1 + height.max(4)
        }
    }
}

pub(super) const TOOLBAR_HEIGHT: i32 = 38;
pub(super) const TOOLBAR_GAP: i32 = 3;
pub(super) const SWATCH: i32 = 26; // 色板色块边长
pub(super) const SWATCH_GAP: i32 = 4;
pub(super) const PALETTE_PAD: i32 = 6; // 色板弹层内边距

// 单行工具栏：PEN / LINE / ARROW / RECT / MOSAIC / TEXT / COLOR / UNDO / COPY / OCR / PIN / SELECT / X
pub(super) const TOOLBAR_ITEM_WIDTHS: [i32; 15] = [34; 15];
pub(super) const TOOLBAR_SLOT_COUNT: usize = 15;
pub(super) const TOOLBAR_SLOT_COLOR: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarItem {
    Tool(Tool),
    /// 色板按钮：点击开关二级色板菜单
    Color,
    Action(ToolbarAction),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolbarAction {
    Copy,
    Ocr,
    Reselect,
    Pin,
    Undo,
    Redo,
    Close,
}

pub(super) fn toolbar_item(slot: usize) -> ToolbarItem {
    match slot {
        0 => ToolbarItem::Tool(Tool::Pen),
        1 => ToolbarItem::Tool(Tool::Line),
        2 => ToolbarItem::Tool(Tool::Arrow),
        3 => ToolbarItem::Tool(Tool::Rect),
        4 => ToolbarItem::Tool(Tool::Mosaic),
        5 => ToolbarItem::Tool(Tool::Text),
        6 => ToolbarItem::Tool(Tool::Select),
        7 => ToolbarItem::Color,
        8 => ToolbarItem::Action(ToolbarAction::Undo),
        9 => ToolbarItem::Action(ToolbarAction::Redo),
        10 => ToolbarItem::Action(ToolbarAction::Copy),
        11 => ToolbarItem::Action(ToolbarAction::Ocr),
        12 => ToolbarItem::Action(ToolbarAction::Pin),
        13 => ToolbarItem::Action(ToolbarAction::Reselect),
        _ => ToolbarItem::Action(ToolbarAction::Close),
    }
}

pub(super) fn toolbar_item_slot(item: ToolbarItem) -> usize {
    match item {
        ToolbarItem::Tool(Tool::Pen) => 0,
        ToolbarItem::Tool(Tool::Line) => 1,
        ToolbarItem::Tool(Tool::Arrow) => 2,
        ToolbarItem::Tool(Tool::Rect) => 3,
        ToolbarItem::Tool(Tool::Mosaic) => 4,
        ToolbarItem::Tool(Tool::Text) => 5,
        ToolbarItem::Tool(Tool::Select) => 6,
        ToolbarItem::Color => 7,
        ToolbarItem::Action(ToolbarAction::Undo) => 8,
        ToolbarItem::Action(ToolbarAction::Redo) => 9,
        ToolbarItem::Action(ToolbarAction::Copy) => 10,
        ToolbarItem::Action(ToolbarAction::Ocr) => 11,
        ToolbarItem::Action(ToolbarAction::Pin) => 12,
        ToolbarItem::Action(ToolbarAction::Reselect) => 13,
        ToolbarItem::Action(ToolbarAction::Close) => 14,
    }
}

pub(super) fn toolbar_size() -> (i32, i32) {
    let width =
        TOOLBAR_ITEM_WIDTHS.iter().sum::<i32>() + TOOLBAR_GAP * (TOOLBAR_SLOT_COUNT as i32 - 1);
    (width, TOOLBAR_HEIGHT)
}

pub(super) fn toolbar_origin(w: i32, h: i32, sel: Option<((i32, i32), (i32, i32))>) -> (i32, i32) {
    let (tw, th) = toolbar_size();
    let (left, top, right, bottom) =
        sel.map(normalized_rect)
            .unwrap_or((w / 2 - 1, h / 2 - 1, w / 2 + 1, h / 2 + 1));
    let max_x = (w - tw - 8).max(8);
    let x = ((left + right - tw) / 2).clamp(8, max_x);
    let y = if bottom + th + 8 <= h {
        bottom + 8
    } else {
        (top - th - 8).max(8)
    };
    (x, y.clamp(8, (h - th - 8).max(8)))
}

pub(super) fn toolbar_item_rect(origin: (i32, i32), slot: usize) -> (i32, i32, i32, i32) {
    let mut x = origin.0;
    for width in TOOLBAR_ITEM_WIDTHS.iter().take(slot) {
        x += width + TOOLBAR_GAP;
    }
    (
        x,
        origin.1,
        x + TOOLBAR_ITEM_WIDTHS[slot],
        origin.1 + TOOLBAR_HEIGHT,
    )
}

pub(super) fn toolbar_hit(
    p: (i32, i32),
    w: i32,
    h: i32,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<ToolbarItem> {
    let origin = toolbar_origin(w, h, sel);
    (0..TOOLBAR_SLOT_COUNT).find_map(|slot| {
        let (x0, y0, x1, y1) = toolbar_item_rect(origin, slot);
        (p.0 >= x0 && p.0 < x1 && p.1 >= y0 && p.1 < y1).then(|| toolbar_item(slot))
    })
}

/// 色板弹层的整体矩形（对齐在色板按钮下，水平居中）。
pub(super) fn palette_size() -> (i32, i32) {
    let w =
        PALETTE.len() as i32 * SWATCH + (PALETTE.len() as i32 - 1) * SWATCH_GAP + PALETTE_PAD * 2;
    (w, SWATCH + PALETTE_PAD * 2)
}

pub(super) fn palette_popup_rect(
    w: i32,
    h: i32,
    color_rect: (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    let (pw, ph) = palette_size();
    let cx = (color_rect.0 + color_rect.2) / 2;
    let x = (cx - pw / 2).clamp(8, (w - pw - 8).max(8));
    let below = color_rect.3 + TOOLBAR_GAP;
    let y = if below + ph <= h {
        below
    } else {
        (color_rect.1 - ph - TOOLBAR_GAP).max(8)
    };
    (x, y, x + pw, y + ph)
}

pub(super) fn palette_swatch_rect(popup: (i32, i32, i32, i32), i: usize) -> (i32, i32, i32, i32) {
    let x = popup.0 + PALETTE_PAD + i as i32 * (SWATCH + SWATCH_GAP);
    (
        x,
        popup.1 + PALETTE_PAD,
        x + SWATCH,
        popup.1 + PALETTE_PAD + SWATCH,
    )
}

pub(super) fn palette_hit(
    p: (i32, i32),
    w: i32,
    h: i32,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<usize> {
    let origin = toolbar_origin(w, h, sel);
    let color_rect = toolbar_item_rect(origin, TOOLBAR_SLOT_COLOR);
    let popup = palette_popup_rect(w, h, color_rect);
    (0..PALETTE.len()).find(|&i| {
        let (x0, y0, x1, y1) = palette_swatch_rect(popup, i);
        p.0 >= x0 && p.0 < x1 && p.1 >= y0 && p.1 < y1
    })
}

#[cfg(test)]
mod text_editing_tests {
    use super::*;

    #[test]
    fn caret_moves_on_utf8_boundaries_and_edits_at_the_insertion_point() {
        let mut editor = EditorState::default();
        editor.start_text((0, 0));
        assert!(editor.insert_text("中b"));
        editor.move_caret_left();
        assert!(editor.insert_text("A"));
        assert!(editor.backspace());
        assert!(editor.insert_text("文"));

        let Shape::Text(_, text) = &editor.annotations[0].shape else {
            panic!("text")
        };
        assert_eq!(text, "中文b");
    }

    #[test]
    fn ime_preedit_deduplication_uses_text_before_the_caret() {
        let mut editor = EditorState::default();
        editor.start_text((0, 0));
        assert!(editor.insert_text("中ni文"));
        editor.move_caret_left();

        assert!(editor.remove_before_caret_if_matches("ni"));
        assert!(editor.insert_text("你"));

        let Shape::Text(_, text) = &editor.annotations[0].shape else {
            panic!("text")
        };
        assert_eq!(text, "中你文");
    }
}

#[cfg(test)]
mod mosaic_editing_tests {
    use super::*;

    #[test]
    fn mosaic_draft_records_a_region_and_discards_zero_area_gestures() {
        let mut editor = EditorState {
            tool: Tool::Mosaic,
            ..EditorState::default()
        };

        editor.start_shape((4, 5));
        editor.update_draft((20, 25));
        editor.commit_draft();

        assert!(matches!(
            editor.annotations.as_slice(),
            [Annotation {
                shape: Shape::Mosaic((4, 5), (20, 25)),
                ..
            }]
        ));

        let mut editor = EditorState {
            tool: Tool::Mosaic,
            ..EditorState::default()
        };

        editor.start_shape((8, 8));
        editor.commit_draft();
        assert!(editor.annotations.is_empty());

        editor.start_shape((8, 8));
        editor.update_draft((20, 8));
        editor.commit_draft();
        editor.start_shape((8, 8));
        editor.update_draft((8, 20));
        editor.commit_draft();
        assert!(editor.annotations.is_empty());
    }
}

#[cfg(test)]
mod annotation_history_tests {
    use super::*;

    #[test]
    fn committed_gestures_undo_redo_and_clear_redo_on_a_new_edit() {
        let mut editor = EditorState {
            tool: Tool::Line,
            ..EditorState::default()
        };

        editor.start_shape((10, 10));
        editor.update_draft((40, 30));
        editor.commit_draft();
        assert_eq!(editor.annotations.len(), 1);

        assert!(editor.undo());
        assert!(editor.annotations.is_empty());
        assert!(editor.redo());
        assert_eq!(editor.annotations.len(), 1);

        assert!(editor.undo());
        editor.start_shape((5, 5));
        editor.update_draft((20, 20));
        editor.commit_draft();
        assert!(!editor.redo());
        assert!(matches!(
            editor.annotations.as_slice(),
            [Annotation {
                shape: Shape::Line((5, 5), (20, 20)),
                ..
            }]
        ));
    }

    #[test]
    fn topmost_visual_annotation_is_selected_moved_and_restored_by_history() {
        let mut editor = EditorState {
            annotations: vec![
                Annotation {
                    shape: Shape::Line((10, 20), (60, 20)),
                    color: PALETTE[0],
                },
                Annotation {
                    shape: Shape::Mosaic((0, 0), (80, 80)),
                    color: PALETTE[1],
                },
            ],
            ..EditorState::default()
        };

        assert_eq!(editor.select_at((30, 20), |_| (0, 0)), Some(0));
        assert!(editor.begin_transform_at((30, 20)));
        assert!(editor.update_transform((40, 35)));
        assert!(editor.commit_transform());
        assert!(matches!(
            editor.annotations[0].shape,
            Shape::Line((20, 35), (70, 35))
        ));

        assert!(editor.undo());
        assert!(matches!(
            editor.annotations[0].shape,
            Shape::Line((10, 20), (60, 20))
        ));
        assert!(editor.redo());
        assert!(matches!(
            editor.annotations[0].shape,
            Shape::Line((20, 35), (70, 35))
        ));
    }

    #[test]
    fn endpoint_and_corner_handles_modify_geometry_as_single_history_steps() {
        let mut editor = EditorState {
            annotations: vec![Annotation {
                shape: Shape::Arrow((10, 10), (50, 10)),
                color: PALETTE[0],
            }],
            selected: Some(0),
            ..EditorState::default()
        };

        assert!(editor.begin_transform_at((50, 10)));
        assert!(editor.update_transform((70, 30)));
        assert!(editor.commit_transform());
        assert!(matches!(
            editor.annotations[0].shape,
            Shape::Arrow((10, 10), (70, 30))
        ));
        assert!(editor.undo());

        editor.annotations = vec![Annotation {
            shape: Shape::Rect((20, 20), (80, 60)),
            color: PALETTE[0],
        }];
        editor.selected = Some(0);
        assert!(editor.begin_transform_at((20, 20)));
        assert!(editor.update_transform((10, 5)));
        assert!(editor.commit_transform());
        assert!(matches!(
            editor.annotations[0].shape,
            Shape::Rect((10, 5), (80, 60))
        ));
    }

    #[test]
    fn selected_color_delete_and_text_reedit_are_history_operations() {
        let mut editor = EditorState {
            annotations: vec![Annotation {
                shape: Shape::Text((20, 30), "hello".into()),
                color: PALETTE[0],
            }],
            selected: Some(0),
            ..EditorState::default()
        };

        assert!(editor.set_color(4));
        assert_eq!(editor.annotations[0].color, PALETTE[4]);
        assert!(editor.undo());
        assert_eq!(editor.annotations[0].color, PALETTE[0]);

        editor.selected = Some(0);
        assert!(editor.reopen_selected_text());
        assert!(editor.insert_text(" world"));
        assert!(editor.commit_text());
        assert!(matches!(
            &editor.annotations[0].shape,
            Shape::Text(_, text) if text == "hello world"
        ));
        assert!(editor.undo());
        assert!(matches!(
            &editor.annotations[0].shape,
            Shape::Text(_, text) if text == "hello"
        ));

        editor.selected = Some(0);
        assert!(editor.delete_selected());
        assert!(editor.annotations.is_empty());
        assert!(editor.undo());
        assert_eq!(editor.annotations.len(), 1);
    }
}
