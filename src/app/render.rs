use super::editor::*;
use super::geometry::normalized_rect;
use super::output::{Annotation, Shape};
use super::windows_adapter::{TEXT_FONT_HEIGHT, gdi_render_text_rgba, gdi_text_size};
use xcap::image::RgbaImage;

/// 把 RGBA 图像铺进 softbuffer 的 0RGB 缓冲。窗口大于图像时，多余区域清黑。
pub(super) fn blit_rgba_image(
    buffer: &mut [u32],
    surface_width: u32,
    surface_height: u32,
    image: &RgbaImage,
) {
    let surface_width = surface_width as usize;
    let surface_height = surface_height as usize;
    let image_width = image.width() as usize;
    let image_height = image.height() as usize;
    let required = surface_width.saturating_mul(surface_height);
    if buffer.len() < required {
        return;
    }
    if image_width != surface_width || image_height != surface_height {
        buffer[..required].fill(0);
    }
    let copy_width = image_width.min(surface_width);
    let raw = image.as_raw();
    for y in 0..image_height.min(surface_height) {
        let source = &raw[y * image_width * 4..(y * image_width + copy_width) * 4];
        let target = &mut buffer[y * surface_width..y * surface_width + copy_width];
        for (x, pixel) in target.iter_mut().enumerate() {
            let i = x * 4;
            *pixel = (source[i] as u32) << 16 | (source[i + 1] as u32) << 8 | source[i + 2] as u32;
        }
    }
}

/// 在像素缓冲上画空心矩形边框，`t` 是线的粗细（像素）。color 是 0RGB 的 u32。
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_rect(
    buf: &mut [u32],
    w: u32,
    h: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
    t: i32,
) {
    let (w, h) = (w as i32, h as i32);
    let left = x0.min(x1).clamp(0, w - 1);
    let right = x0.max(x1).clamp(0, w - 1);
    let top = y0.min(y1).clamp(0, h - 1);
    let bottom = y0.max(y1).clamp(0, h - 1);
    let t = t.max(1);
    // 上、下两条横边（各 t 像素厚，往内叠）
    for d in 0..t {
        let yt = (top + d).min(h - 1);
        let yb = (bottom - d).max(0);
        for x in left..=right {
            buf[(yt * w + x) as usize] = color;
            buf[(yb * w + x) as usize] = color;
        }
    }
    // 左、右两条竖边
    for d in 0..t {
        let xl = (left + d).min(w - 1);
        let xr = (right - d).max(0);
        for y in top..=bottom {
            buf[(y * w + xl) as usize] = color;
            buf[(y * w + xr) as usize] = color;
        }
    }
}

pub(super) fn draw_fill_rect(
    buf: &mut [u32],
    w: u32,
    h: u32,
    rect: (i32, i32, i32, i32),
    color: u32,
) {
    let (x0, y0, x1, y1) = rect;
    let left = x0.clamp(0, w as i32);
    let top = y0.clamp(0, h as i32);
    let right = x1.clamp(0, w as i32);
    let bottom = y1.clamp(0, h as i32);
    for y in top..bottom {
        let start = (y as u32 * w + left as u32) as usize;
        let end = (y as u32 * w + right as u32) as usize;
        if start < end && end <= buf.len() {
            buf[start..end].fill(color);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_toolbar(
    buf: &mut [u32],
    w: u32,
    h: u32,
    sel: Option<((i32, i32), (i32, i32))>,
    tool: Tool,
    color: [u8; 4],
    hover: Option<usize>,
    palette_open: bool,
) {
    let origin = toolbar_origin(w as i32, h as i32, sel);
    let (tw, th) = toolbar_size();
    draw_fill_rect(
        buf,
        w,
        h,
        (
            origin.0 + 2,
            origin.1 + 3,
            origin.0 + tw + 2,
            origin.1 + th + 3,
        ),
        0x00101010,
    );
    draw_fill_rect(
        buf,
        w,
        h,
        (origin.0, origin.1, origin.0 + tw, origin.1 + th),
        0x00212631,
    );
    for slot in 0..TOOLBAR_SLOT_COUNT {
        let rect = toolbar_item_rect(origin, slot);
        let item = toolbar_item(slot);
        let mut fill = match item {
            ToolbarItem::Tool(t) => {
                if t == tool {
                    0x00D88928
                } else {
                    0x004B5968
                }
            }
            ToolbarItem::Color => 0x003B78C8,
            ToolbarItem::Action(ToolbarAction::Copy) => 0x002D9B68,
            ToolbarItem::Action(ToolbarAction::Ocr) => 0x00704AA3,
            ToolbarItem::Action(ToolbarAction::Reselect) => 0x006B5AA8,
            ToolbarItem::Action(ToolbarAction::Pin) => 0x003B78C8,
            ToolbarItem::Action(ToolbarAction::Undo) => 0x00515D6B,
            ToolbarItem::Action(ToolbarAction::Close) => 0x00A83D48,
        };
        if hover == Some(slot) {
            fill = match item {
                ToolbarItem::Action(ToolbarAction::Close) => 0x00D85A65,
                _ => 0x006D91B5,
            };
        }
        draw_fill_rect(buf, w, h, rect, fill);
        // 高亮：选中的工具 / 打开中的色板按钮
        match item {
            ToolbarItem::Tool(t) if t == tool => {
                draw_rect(
                    buf,
                    w,
                    h,
                    rect.0,
                    rect.1,
                    rect.2 - 1,
                    rect.3 - 1,
                    0x00FFFFFF,
                    2,
                );
            }
            ToolbarItem::Color if palette_open => {
                draw_rect(
                    buf,
                    w,
                    h,
                    rect.0,
                    rect.1,
                    rect.2 - 1,
                    rect.3 - 1,
                    0x00FFFFFF,
                    2,
                );
            }
            _ => {}
        }
        draw_rect(
            buf,
            w,
            h,
            rect.0,
            rect.1,
            rect.2 - 1,
            rect.3 - 1,
            0x00D9E2EC,
            1,
        );
        match item {
            // 色板按钮：居中画当前颜色方块 + 下方下拉箭头
            ToolbarItem::Color => {
                let s = 18;
                let sx = rect.0 + (rect.2 - rect.0 - s) / 2;
                let sy = rect.1 + 7;
                draw_fill_rect(buf, w, h, (sx, sy, sx + s, sy + s), color_u32(color));
                draw_rect(buf, w, h, sx, sy, sx + s - 1, sy + s - 1, 0x00FFFFFF, 1);
                let cx = (rect.0 + rect.2) / 2;
                let ay = rect.3 - 9;
                for d in 0..4 {
                    let half = 3 - d;
                    draw_fill_rect(
                        buf,
                        w,
                        h,
                        (cx - half, ay + d, cx + half + 1, ay + d + 1),
                        0x00FFFFFF,
                    );
                }
            }
            _ => {
                let label = match item {
                    ToolbarItem::Tool(Tool::Pen) => "PEN",
                    ToolbarItem::Tool(Tool::Line) => "LINE",
                    ToolbarItem::Tool(Tool::Rect) => "RECT",
                    ToolbarItem::Tool(Tool::Text) => "TEXT",
                    ToolbarItem::Action(ToolbarAction::Copy) => "COPY",
                    ToolbarItem::Action(ToolbarAction::Ocr) => "OCR",
                    ToolbarItem::Action(ToolbarAction::Reselect) => "SELECT",
                    ToolbarItem::Action(ToolbarAction::Pin) => "PIN",
                    ToolbarItem::Action(ToolbarAction::Undo) => "UNDO",
                    ToolbarItem::Action(ToolbarAction::Close) => "X",
                    _ => "",
                };
                let text_width = (label.chars().count() as i32 * 12) - 2;
                draw_text(
                    buf,
                    w,
                    h,
                    rect.0 + (rect.2 - rect.0 - text_width) / 2,
                    rect.1 + 10,
                    label,
                    2,
                    0x00FFFFFF,
                );
            }
        }
    }
}

/// 画打开的二级色板菜单：8 个色块一排，当前颜色白框、悬停蓝框。
pub(super) fn draw_palette_popup(
    buf: &mut [u32],
    w: u32,
    h: u32,
    sel: Option<((i32, i32), (i32, i32))>,
    color: [u8; 4],
    hover: Option<usize>,
) {
    let origin = toolbar_origin(w as i32, h as i32, sel);
    let color_rect = toolbar_item_rect(origin, TOOLBAR_SLOT_COLOR);
    let popup = palette_popup_rect(w as i32, h as i32, color_rect);
    draw_fill_rect(
        buf,
        w,
        h,
        (popup.0 + 2, popup.1 + 3, popup.2 + 2, popup.3 + 3),
        0x00101010,
    );
    draw_fill_rect(buf, w, h, popup, 0x00212631);
    for (i, swatch) in PALETTE.iter().enumerate() {
        let rect = palette_swatch_rect(popup, i);
        draw_fill_rect(buf, w, h, rect, color_u32(*swatch));
        if *swatch == color {
            draw_rect(
                buf,
                w,
                h,
                rect.0 - 1,
                rect.1 - 1,
                rect.2,
                rect.3,
                0x00FFFFFF,
                2,
            );
        } else if hover == Some(i) {
            draw_rect(
                buf,
                w,
                h,
                rect.0 - 1,
                rect.1 - 1,
                rect.2,
                rect.3,
                0x006D91B5,
                2,
            );
        }
        draw_rect(
            buf,
            w,
            h,
            rect.0,
            rect.1,
            rect.2 - 1,
            rect.3 - 1,
            0x00D9E2EC,
            1,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_text(
    buf: &mut [u32],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    text: &str,
    scale: i32,
    color: u32,
) {
    let mut cursor_x = x;
    for ch in text.chars() {
        let rows = glyph(ch);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    draw_fill_rect(
                        buf,
                        w,
                        h,
                        (
                            cursor_x + col * scale,
                            y + row as i32 * scale,
                            cursor_x + (col + 1) * scale,
                            y + (row as i32 + 1) * scale,
                        ),
                        color,
                    );
                }
            }
        }
        cursor_x += 5 * scale + scale;
    }
}

pub(super) fn glyph(ch: char) -> [u8; 7] {
    match ch {
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'N' => [
            0b10001, 0b11001, 0b11001, 0b10101, 0b10011, 0b10011, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        _ => [0; 7],
    }
}

pub(super) fn draw_pin_badge(buf: &mut [u32], w: u32, h: u32) {
    draw_fill_rect(buf, w, h, (8, 8, 50, 36), 0x003B78C8);
    draw_text(buf, w, h, 14, 18, "PIN", 2, 0x00FFFFFF);
}

pub(super) fn draw_select_badge(buf: &mut [u32], w: u32, h: u32) {
    let rect = (8, 8, 108, 36);
    draw_fill_rect(buf, w, h, rect, 0x003B78C8);
    draw_rect(
        buf,
        w,
        h,
        rect.0,
        rect.1,
        rect.2 - 1,
        rect.3 - 1,
        0x00FFFFFF,
        1,
    );
    draw_text(buf, w, h, 16, 18, "SELECT", 2, 0x00FFFFFF);
}

pub(super) fn shade_outside(buf: &mut [u32], w: u32, h: u32, a: (i32, i32), b: (i32, i32)) {
    let (left, top, right, bottom) = normalized_rect((a, b));
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if x < left || x > right || y < top || y > bottom {
                let i = (y as u32 * w + x as u32) as usize;
                let c = buf[i];
                buf[i] = ((c & 0x00FEFEFE) >> 1) & 0x007F7F7F;
            }
        }
    }
}

pub(super) fn draw_line_buffer(
    buf: &mut [u32],
    w: u32,
    h: u32,
    a: (i32, i32),
    b: (i32, i32),
    color: u32,
    radius: i32,
) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let steps = dx.abs().max(dy.abs()).max(1);
    for i in 0..=steps {
        let x = a.0 + dx * i / steps;
        let y = a.1 + dy * i / steps;
        for oy in -radius..=radius {
            for ox in -radius..=radius {
                if ox * ox + oy * oy <= radius * radius {
                    let (px, py) = (x + ox, y + oy);
                    if px >= 0 && py >= 0 && px < w as i32 && py < h as i32 {
                        buf[(py as u32 * w + px as u32) as usize] = color;
                    }
                }
            }
        }
    }
}

/// RGBA 颜色 → 显示缓冲用的 0RGB u32。
pub(super) fn color_u32(c: [u8; 4]) -> u32 {
    (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// 把 RGBA 子图以 source-over 混合到显示缓冲（0RGB，无 alpha）。
#[allow(clippy::too_many_arguments)]
pub(super) fn blend_rgba_buffer(
    buf: &mut [u32],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    rgba: &[u8],
    tw: i32,
    th: i32,
) {
    for j in 0..th {
        for i in 0..tw {
            let idx = ((j * tw + i) * 4) as usize;
            let a = rgba[idx + 3] as u32;
            if a == 0 {
                continue;
            }
            let (px, py) = (x + i, y + j);
            if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                continue;
            }
            let dst = buf[(py as u32 * w + px as u32) as usize];
            let inv = 255 - a;
            let or = (rgba[idx] as u32 * a + ((dst >> 16) & 0xFF) * inv) / 255;
            let og = (rgba[idx + 1] as u32 * a + ((dst >> 8) & 0xFF) * inv) / 255;
            let ob = (rgba[idx + 2] as u32 * a + (dst & 0xFF) * inv) / 255;
            buf[(py as u32 * w + px as u32) as usize] = (or << 16) | (og << 8) | ob;
        }
    }
}

pub(super) fn draw_text_buffer(
    buf: &mut [u32],
    w: u32,
    h: u32,
    text: &str,
    pos: (i32, i32),
    color: [u8; 4],
) {
    if let Some((tw, th, rgba)) = gdi_render_text_rgba(text, color) {
        blend_rgba_buffer(buf, w, h, pos.0, pos.1, &rgba, tw, th);
    }
}

/// 画文字输入提示：组合拼音（浅色+下划线）+ 闪烁光标。不画外边框。
pub(super) fn draw_text_edit_box(
    buf: &mut [u32],
    w: u32,
    h: u32,
    ann: &Annotation,
    preedit: &str,
    cursor_visible: bool,
) {
    const CARET_COLOR: u32 = 0x004C9AFF; // 亮蓝：在任何底色上都醒目
    if let Shape::Text(pos, text) = &ann.shape {
        let full = format!("{text}{preedit}");
        let (tw, th) = gdi_text_size(&full);
        let (tw, th) = (tw.max(4), th.max(TEXT_FONT_HEIGHT));
        // 组合中的拼音：画在已提交文字后面，用浅色 + 下划线区分
        if !preedit.is_empty() {
            let (tw0, _) = gdi_text_size(text);
            let lighter = [
                ann.color[0] + (255 - ann.color[0]) / 2,
                ann.color[1] + (255 - ann.color[1]) / 2,
                ann.color[2] + (255 - ann.color[2]) / 2,
                255,
            ];
            draw_text_buffer(buf, w, h, preedit, (pos.0 + tw0, pos.1), lighter);
            draw_line_buffer(
                buf,
                w,
                h,
                (pos.0 + tw0, pos.1 + th - 2),
                (pos.0 + tw, pos.1 + th - 2),
                color_u32(lighter),
                0,
            );
        }
        // 闪烁光标：3px 宽实心竖条，紧跟文字末尾
        if cursor_visible {
            let cx = pos.0 + tw;
            draw_line_buffer(
                buf,
                w,
                h,
                (cx, pos.1 + 2),
                (cx, pos.1 + th - 2),
                CARET_COLOR,
                1,
            );
        }
    }
}
