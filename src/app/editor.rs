use super::geometry::normalized_rect;
use super::state::Mode;

/// 当前选中的标注工具（编辑模式下左键拖拽用哪个图元）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Pen,
    Line,
    Rect,
    Text,
}

impl Default for Tool {
    fn default() -> Self {
        Tool::Pen
    }
}

/// 一条标注的形状：自由画笔（一串点）/ 直线（两点）/ 矩形（两对角点）/ 文字（左上角锚点 + 内容）。
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Shape {
    Pen(Vec<(i32, i32)>),
    Line((i32, i32), (i32, i32)),
    Rect((i32, i32), (i32, i32)),
    Text((i32, i32), String),
}

/// 一条标注 = 形状 + 颜色（RGBA，输出图直接用；显示缓冲按 0RGB 转换）。
#[derive(Clone, Debug)]
pub(super) struct Annotation {
    pub(super) shape: Shape,
    pub(super) color: [u8; 4],
}

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
#[derive(Default)]
pub(super) struct EditorState {
    pub(super) mode: Mode,
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
}

impl EditorState {
    pub(super) fn reset_for_capture(&mut self) {
        self.mode = Mode::Selecting;
        self.annotations.clear();
        self.tool = Tool::Pen;
        self.color = PALETTE[0];
        self.drawing = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        self.text_editing = false;
        self.ime_preedit.clear();
        self.close_palette();
    }

    pub(super) fn reset_for_reselect(&mut self) {
        self.mode = Mode::Selecting;
        self.annotations.clear();
        self.drawing = false;
        self.toolbar_hover = None;
        self.toolbar_pressed = None;
        self.text_editing = false;
        self.ime_preedit.clear();
        self.close_palette();
    }

    pub(super) fn close_palette(&mut self) {
        self.palette_open = false;
        self.palette_hover = None;
        self.palette_pressed = None;
    }

    pub(super) fn set_color(&mut self, index: usize) {
        let color = PALETTE[index];
        self.color = color;
        if self.text_editing {
            if let Some(last) = self.annotations.last_mut() {
                if matches!(last.shape, Shape::Text(..)) {
                    last.color = color;
                }
            }
        }
    }

    pub(super) fn start_shape(&mut self, point: (i32, i32)) {
        let shape = match self.tool {
            Tool::Pen => Shape::Pen(vec![point]),
            Tool::Line => Shape::Line(point, point),
            Tool::Rect => Shape::Rect(point, point),
            Tool::Text => return,
        };
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
            Shape::Line(_, end) | Shape::Rect(_, end) => *end = point,
            Shape::Text(..) => {}
        }
    }

    pub(super) fn commit_draft(&mut self) {
        let Some(annotation) = self.annotations.last() else {
            return;
        };
        let (drop, dot) = match &annotation.shape {
            Shape::Pen(points) => (false, points.len() == 1),
            Shape::Line(start, end) | Shape::Rect(start, end) => (start == end, false),
            Shape::Text(..) => (false, false),
        };
        if drop {
            self.annotations.pop();
        } else if dot {
            if let Some(Shape::Pen(points)) = self.annotations.last_mut().map(|a| &mut a.shape) {
                points.push(points[0]);
            }
        }
    }

    pub(super) fn start_text(&mut self, point: (i32, i32)) {
        self.commit_text();
        self.annotations.push(Annotation {
            shape: Shape::Text(point, String::new()),
            color: self.color,
        });
        self.text_editing = true;
        self.ime_preedit.clear();
        self.cursor_visible = true;
    }

    pub(super) fn commit_text(&mut self) -> bool {
        if !self.text_editing {
            return false;
        }
        self.text_editing = false;
        self.ime_preedit.clear();
        if self
            .annotations
            .last()
            .is_some_and(|last| matches!(&last.shape, Shape::Text(_, text) if text.is_empty()))
        {
            self.annotations.pop();
        }
        true
    }

    pub(super) fn cancel_text(&mut self) -> bool {
        if !self.text_editing {
            return false;
        }
        self.text_editing = false;
        self.ime_preedit.clear();
        if self
            .annotations
            .last()
            .is_some_and(|last| matches!(last.shape, Shape::Text(..)))
        {
            self.annotations.pop();
        }
        true
    }
}

pub(super) const TOOLBAR_HEIGHT: i32 = 38;
pub(super) const TOOLBAR_GAP: i32 = 4;
pub(super) const SWATCH: i32 = 26; // 色板色块边长
pub(super) const SWATCH_GAP: i32 = 4;
pub(super) const PALETTE_PAD: i32 = 6; // 色板弹层内边距

// 单行工具栏：PEN / LINE / RECT / TEXT / COLOR / UNDO / COPY / OCR / PIN / SELECT / X
pub(super) const TOOLBAR_ITEM_WIDTHS: [i32; 11] = [46, 50, 50, 50, 44, 50, 50, 42, 44, 74, 30];
pub(super) const TOOLBAR_SLOT_COUNT: usize = 11;
pub(super) const TOOLBAR_SLOT_COLOR: usize = 4;

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
    Close,
}

pub(super) fn toolbar_item(slot: usize) -> ToolbarItem {
    match slot {
        0 => ToolbarItem::Tool(Tool::Pen),
        1 => ToolbarItem::Tool(Tool::Line),
        2 => ToolbarItem::Tool(Tool::Rect),
        3 => ToolbarItem::Tool(Tool::Text),
        4 => ToolbarItem::Color,
        5 => ToolbarItem::Action(ToolbarAction::Undo),
        6 => ToolbarItem::Action(ToolbarAction::Copy),
        7 => ToolbarItem::Action(ToolbarAction::Ocr),
        8 => ToolbarItem::Action(ToolbarAction::Pin),
        9 => ToolbarItem::Action(ToolbarAction::Reselect),
        _ => ToolbarItem::Action(ToolbarAction::Close),
    }
}

pub(super) fn toolbar_item_slot(item: ToolbarItem) -> usize {
    match item {
        ToolbarItem::Tool(Tool::Pen) => 0,
        ToolbarItem::Tool(Tool::Line) => 1,
        ToolbarItem::Tool(Tool::Rect) => 2,
        ToolbarItem::Tool(Tool::Text) => 3,
        ToolbarItem::Color => 4,
        ToolbarItem::Action(ToolbarAction::Undo) => 5,
        ToolbarItem::Action(ToolbarAction::Copy) => 6,
        ToolbarItem::Action(ToolbarAction::Ocr) => 7,
        ToolbarItem::Action(ToolbarAction::Pin) => 8,
        ToolbarItem::Action(ToolbarAction::Reselect) => 9,
        ToolbarItem::Action(ToolbarAction::Close) => 10,
    }
}

pub(super) fn toolbar_size() -> (i32, i32) {
    let w = TOOLBAR_ITEM_WIDTHS.iter().sum::<i32>() + TOOLBAR_GAP * (TOOLBAR_SLOT_COUNT as i32 - 1);
    (w, TOOLBAR_HEIGHT)
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
    for i in 0..slot {
        x += TOOLBAR_ITEM_WIDTHS[i] + TOOLBAR_GAP;
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
    (0..PALETTE.len()).find_map(|i| {
        let (x0, y0, x1, y1) = palette_swatch_rect(popup, i);
        (p.0 >= x0 && p.0 < x1 && p.1 >= y0 && p.1 < y1).then_some(i)
    })
}
