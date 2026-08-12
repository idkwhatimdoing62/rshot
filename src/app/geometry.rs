use xcap::image::{RgbaImage, imageops};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RectI {
    pub(super) left: i32,
    pub(super) top: i32,
    pub(super) right: i32,
    pub(super) bottom: i32,
}

pub(super) fn normalized_rect((a, b): ((i32, i32), (i32, i32))) -> (i32, i32, i32, i32) {
    (a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

pub(super) fn point_in_selection(p: (i32, i32), sel: Option<((i32, i32), (i32, i32))>) -> bool {
    sel.map(normalized_rect)
        .is_some_and(|r| p.0 >= r.0 && p.0 <= r.2 && p.1 >= r.1 && p.1 <= r.3)
}

pub(super) fn selection_has_area(sel: Option<((i32, i32), (i32, i32))>) -> bool {
    sel.map(normalized_rect)
        .is_some_and(|(left, top, right, bottom)| right > left && bottom > top)
}

/// 按对角两点 a、b 从原图裁出子矩形，进剪贴板。零尺寸就跳过。
pub(super) fn crop_image(img: &RgbaImage, a: (i32, i32), b: (i32, i32)) -> Option<RgbaImage> {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let left = a.0.min(b.0).clamp(0, iw);
    let right = a.0.max(b.0).clamp(0, iw);
    let top = a.1.min(b.1).clamp(0, ih);
    let bottom = a.1.max(b.1).clamp(0, ih);
    let (bw, bh) = ((right - left) as u32, (bottom - top) as u32);
    if bw == 0 || bh == 0 {
        return None;
    }
    Some(imageops::crop_imm(img, left as u32, top as u32, bw, bh).to_image())
}
