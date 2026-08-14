use super::geometry::normalized_rect;
use std::borrow::Cow;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use xcap::image::{RgbImage, RgbaImage, imageops};

const OCR_MIN_TARGET_HEIGHT: u32 = 600;
const OCR_MAX_UPSCALE: u32 = 2;
const OCR_UPSCALE_PIXEL_BUDGET: u64 = 2_000_000;
const MODEL_OCR_MAX_DIMENSION: u32 = 4096;
const MODEL_OCR_PIXEL_BUDGET: u64 = 8_000_000;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct OcrWordData {
    pub(super) text: String,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct OcrLineData {
    pub(super) words: Vec<OcrWordData>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct OcrRegionData {
    pub(super) text: String,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) space_before: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct OcrCharacterData {
    pub(super) ch: char,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OcrBackend {
    PpOcrV6,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OcrFallbackReason {
    ModelReturnedNoText,
    ModelUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OcrRecognition {
    pub(super) text: String,
    pub(super) backend: OcrBackend,
    pub(super) fallback_reason: Option<OcrFallbackReason>,
}

/// 计算 OCR 使用的原图区域。返回 left / top / width / height，全部限制在图片内。
pub(super) fn ocr_region(
    img: &RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<(u32, u32, u32, u32)> {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let (left, top, right, bottom) = sel.map(normalized_rect).unwrap_or((0, 0, iw, ih));
    let left = left.clamp(0, iw);
    let right = right.clamp(0, iw);
    let top = top.clamp(0, ih);
    let bottom = bottom.clamp(0, ih);
    (right > left && bottom > top).then_some((
        left as u32,
        top as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// 从原始截图提取 OCR 输入，并在超过系统上限时等比缩小。这样不会识别用户画的标注。
pub(super) fn prepare_ocr_rgba<'a>(
    img: &'a RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
    max_dimension: u32,
) -> Option<(Cow<'a, [u8]>, u32, u32)> {
    if max_dimension == 0 {
        return None;
    }
    let (left, top, width, height) = ocr_region(img, sel)?;
    let largest = width.max(height);
    if largest > max_dimension {
        let scaled_width = ((width as u64 * max_dimension as u64) / largest as u64).max(1) as u32;
        let scaled_height = ((height as u64 * max_dimension as u64) / largest as u64).max(1) as u32;
        let view = imageops::crop_imm(img, left, top, width, height);
        let resized = imageops::resize(
            &*view,
            scaled_width,
            scaled_height,
            imageops::FilterType::Triangle,
        );
        return Some((Cow::Owned(resized.into_raw()), scaled_width, scaled_height));
    }

    if left == 0 && top == 0 && width == img.width() && height == img.height() {
        return Some((Cow::Borrowed(img.as_raw()), width, height));
    }

    // 未缩放时按行直接复制选区，避免先创建一个 RgbaImage 再复制进 WinRT 缓冲区。
    let source_stride = img.width() as usize * 4;
    let row_bytes = width as usize * 4;
    let raw = img.as_raw();
    let mut rgba = Vec::with_capacity(row_bytes * height as usize);
    for y in top..top + height {
        let start = y as usize * source_stride + left as usize * 4;
        rgba.extend_from_slice(&raw[start..start + row_bytes]);
    }
    Some((Cow::Owned(rgba), width, height))
}

/// 小尺寸文本截图先放大 2 倍；实测能显著减少简体中文和标点误识别。
/// 大图保持原样，超过系统上限时仍按比例缩小，避免无界增加 OCR 峰值内存。
pub(super) fn prepare_ocr_rgba_for_recognition<'a>(
    img: &'a RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
    max_dimension: u32,
) -> Option<(Cow<'a, [u8]>, u32, u32)> {
    let (rgba, width, height) = prepare_ocr_rgba(img, sel, max_dimension)?;
    let largest = width.max(height);
    let scaled_pixels = width as u64 * height as u64 * (OCR_MAX_UPSCALE as u64).pow(2);
    let scale = if height < OCR_MIN_TARGET_HEIGHT
        && largest <= max_dimension / OCR_MAX_UPSCALE
        && scaled_pixels <= OCR_UPSCALE_PIXEL_BUDGET
    {
        OCR_MAX_UPSCALE
    } else {
        1
    };
    if scale == 1 {
        return Some((rgba, width, height));
    }

    let source = RgbaImage::from_raw(width, height, rgba.into_owned())?;
    let resized = imageops::resize(
        &source,
        width * scale,
        height * scale,
        imageops::FilterType::Triangle,
    );
    Some((
        Cow::Owned(resized.into_raw()),
        width * scale,
        height * scale,
    ))
}

/// 为内置模型准备原始选区。PP-OCR 自带尺寸归一化，因此这里只在输入过大时
/// 等比缩小，不再先做会损伤灰底代码片段的二值化或锐化。
pub(super) fn prepare_ocr_worker_rgba(
    img: &RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Option<(Vec<u8>, u32, u32)> {
    let (rgba, width, height) = prepare_ocr_rgba(img, sel, MODEL_OCR_MAX_DIMENSION)?;
    let pixels = width as u64 * height as u64;
    if pixels <= MODEL_OCR_PIXEL_BUDGET {
        return Some((rgba.into_owned(), width, height));
    }

    let scale = (MODEL_OCR_PIXEL_BUDGET as f64 / pixels as f64).sqrt();
    let mut scaled_width = ((width as f64 * scale).floor() as u32).max(1);
    let mut scaled_height = ((height as f64 * scale).floor() as u32).max(1);
    while scaled_width as u64 * scaled_height as u64 > MODEL_OCR_PIXEL_BUDGET {
        if scaled_width >= scaled_height && scaled_width > 1 {
            scaled_width -= 1;
        } else if scaled_height > 1 {
            scaled_height -= 1;
        } else {
            break;
        }
    }
    let source = RgbaImage::from_raw(width, height, rgba.into_owned())?;
    let resized = imageops::resize(
        &source,
        scaled_width,
        scaled_height,
        imageops::FilterType::Triangle,
    );
    Some((resized.into_raw(), scaled_width, scaled_height))
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x2FFF
            | 0x3040..=0x30FF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xAC00..=0xD7AF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

fn is_cjk_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '、' | '。'
            | '，'
            | '；'
            | '：'
            | '！'
            | '？'
            | '（'
            | '）'
            | '【'
            | '】'
            | '《'
            | '》'
            | '〈'
            | '〉'
            | '「'
            | '」'
            | '『'
            | '』'
            | '…'
            | '—'
            | '～'
            | '·'
            | '•'
            | '●'
            | '∙'
    )
}

fn is_opening_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '(' | '[' | '{' | '（' | '【' | '《' | '〈' | '「' | '『' | '“'
    )
}

fn is_closing_punctuation(ch: char) -> bool {
    matches!(
        ch,
        ')' | ']'
            | '}'
            | ','
            | '.'
            | ';'
            | ':'
            | '!'
            | '?'
            | '）'
            | '】'
            | '》'
            | '〉'
            | '」'
            | '』'
            | '”'
            | '、'
            | '。'
            | '，'
            | '；'
            | '：'
            | '！'
            | '？'
    )
}

fn looks_like_flat_dash(word: &OcrWordData) -> bool {
    matches!(word.text.as_str(), "一" | "·" | "•" | "∙")
        && word.width >= word.height * 1.5
        && word.height <= 5.0
}

fn is_short_ascii_identifier(word: &OcrWordData) -> bool {
    !word.text.is_empty()
        && word.text.len() <= 4
        && word.text.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn is_ascii_digits(word: &OcrWordData) -> bool {
    !word.text.is_empty() && word.text.chars().all(|ch| ch.is_ascii_digit())
}

fn horizontal_gap(left: &OcrWordData, right: &OcrWordData) -> f32 {
    (right.x - left.x - left.width).max(0.0)
}

fn tight_gap_baseline(words: &[&OcrWordData], line_height: f32) -> f32 {
    let mut gaps: Vec<f32> = words
        .windows(2)
        .map(|pair| horizontal_gap(pair[0], pair[1]))
        .filter(|gap| *gap <= line_height * 0.24)
        .collect();
    if gaps.is_empty() {
        return line_height * 0.05;
    }
    gaps.sort_by(f32::total_cmp);
    gaps[gaps.len() / 2]
}

fn same_word_row(group: &[OcrWordData], word: &OcrWordData) -> bool {
    let top = group
        .iter()
        .map(|item| item.y)
        .fold(f32::INFINITY, f32::min);
    let bottom = group
        .iter()
        .map(|item| item.y + item.height)
        .fold(f32::NEG_INFINITY, f32::max);
    let center = group
        .iter()
        .map(|item| item.y + item.height * 0.5)
        .sum::<f32>()
        / group.len() as f32;
    let word_center = word.y + word.height * 0.5;
    let overlap = bottom.min(word.y + word.height) - top.max(word.y);
    overlap >= word.height.min(bottom - top).max(1.0) * 0.35
        || (center - word_center).abs() <= word.height.max(bottom - top).max(1.0) * 0.40
}

/// Windows OCR 有时会把同一视觉行的灰底代码片段拆成多条并乱序返回。
/// 这里把所有词按物理坐标重新聚类，超大横向间隔仍拆开，避免误合并双栏内容。
pub(super) fn regroup_ocr_lines(lines: &[OcrLineData]) -> Vec<OcrLineData> {
    let mut words: Vec<OcrWordData> = lines
        .iter()
        .flat_map(|line| line.words.iter())
        .filter(|word| !word.text.trim().is_empty() && word.width > 0.0 && word.height > 0.0)
        .cloned()
        .collect();
    words.sort_by(|left, right| {
        (left.y + left.height * 0.5)
            .total_cmp(&(right.y + right.height * 0.5))
            .then_with(|| left.x.total_cmp(&right.x))
    });

    let mut rows: Vec<Vec<OcrWordData>> = Vec::new();
    for word in words {
        if let Some(row) = rows.iter_mut().rev().find(|row| same_word_row(row, &word)) {
            row.push(word);
        } else {
            rows.push(vec![word]);
        }
    }

    let mut rebuilt = Vec::new();
    for mut row in rows {
        row.sort_by(|left, right| left.x.total_cmp(&right.x));
        let mut segment = Vec::new();
        for word in row {
            let starts_new_segment = segment.last().is_some_and(|previous: &OcrWordData| {
                word.x - previous.x - previous.width
                    > previous.height.max(word.height).max(1.0) * 3.0
            });
            if starts_new_segment {
                rebuilt.push(OcrLineData {
                    words: std::mem::take(&mut segment),
                });
            }
            segment.push(word);
        }
        if !segment.is_empty() {
            rebuilt.push(OcrLineData { words: segment });
        }
    }
    rebuilt.sort_by(|left, right| {
        let left_y = left
            .words
            .iter()
            .map(|word| word.y)
            .fold(f32::INFINITY, f32::min);
        let right_y = right
            .words
            .iter()
            .map(|word| word.y)
            .fold(f32::INFINITY, f32::min);
        let left_x = left
            .words
            .iter()
            .map(|word| word.x)
            .fold(f32::INFINITY, f32::min);
        let right_x = right
            .words
            .iter()
            .map(|word| word.x)
            .fold(f32::INFINITY, f32::min);
        left_y
            .total_cmp(&right_y)
            .then_with(|| left_x.total_cmp(&right_x))
    });
    rebuilt
}

fn is_tight_identifier_dash(words: &[&OcrWordData], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).and_then(|i| words.get(i)).copied() else {
        return false;
    };
    let Some(current) = words.get(index).copied() else {
        return false;
    };
    let Some(next) = words.get(index + 1).copied() else {
        return false;
    };
    let text_height = previous.height.max(next.height).max(1.0);
    (current.text == "-" || looks_like_flat_dash(current))
        && is_short_ascii_identifier(previous)
        && is_ascii_digits(next)
        && horizontal_gap(previous, current) <= text_height * 0.20
        && horizontal_gap(current, next) <= text_height * 0.20
}

fn normalize_identifier_dash(words: &[&OcrWordData], index: usize) -> Option<&'static str> {
    is_tight_identifier_dash(words, index).then_some("-")
}

fn current_follows_identifier_dash(words: &[&OcrWordData], index: usize) -> bool {
    let Some(dash_index) = index.checked_sub(1) else {
        return false;
    };
    is_tight_identifier_dash(words, dash_index)
}

fn previous_ends_ascii_identifier(words: &[&OcrWordData], index: usize) -> bool {
    let Some(dash_index) = index.checked_sub(2) else {
        return false;
    };
    is_tight_identifier_dash(words, dash_index)
}

fn needs_space_after_ascii_identifier(
    words: &[&OcrWordData],
    index: usize,
    line_height: f32,
) -> bool {
    let Some(current) = words.get(index).copied() else {
        return false;
    };
    let Some(previous) = index.checked_sub(1).and_then(|i| words.get(i)).copied() else {
        return false;
    };
    let Some(first) = current.text.chars().next() else {
        return false;
    };
    previous_ends_ascii_identifier(words, index)
        && horizontal_gap(previous, current) > line_height * 0.25
        && !is_cjk_punctuation(first)
        && !is_closing_punctuation(first)
        && first != '~'
}

fn needs_space(
    previous: &OcrWordData,
    current: &OcrWordData,
    line_height: f32,
    tight_gap: f32,
) -> bool {
    let gap = horizontal_gap(previous, current);
    let Some(left) = previous.text.chars().next_back() else {
        return false;
    };
    let Some(right) = current.text.chars().next() else {
        return false;
    };
    if is_opening_punctuation(left)
        || is_closing_punctuation(right)
        || is_cjk_punctuation(left)
        || is_cjk_punctuation(right)
    {
        return false;
    }
    // Windows 已把拉丁文本拆成两个词时，即使字体的空格很窄，也应保留词边界。
    // 极小阈值仍允许 OCR 偶尔把一个无空格标识符误拆成相邻词时重新拼合。
    if left.is_ascii_alphanumeric() && right.is_ascii_alphanumeric() {
        return gap > line_height * 0.08;
    }
    if (left.is_ascii_digit() && is_cjk(right)) || (is_cjk(left) && right.is_ascii_digit()) {
        return gap > (line_height * 0.26).max(tight_gap + line_height * 0.12);
    }
    if gap <= line_height * 0.25 {
        return false;
    }
    if gap >= line_height * 0.45 {
        return true;
    }

    let left_cjk_or_digit = is_cjk(left) || left.is_ascii_digit();
    let right_cjk_or_digit = is_cjk(right) || right.is_ascii_digit();
    !(left_cjk_or_digit && right_cjk_or_digit)
}

pub(super) fn rebuild_ocr_text(lines: &[OcrLineData]) -> String {
    let mut output = Vec::with_capacity(lines.len());
    for line in lines {
        let words: Vec<&OcrWordData> = line
            .words
            .iter()
            .filter(|word| !word.text.trim().is_empty())
            .collect();
        if words.is_empty() {
            continue;
        }
        let line_height = words
            .iter()
            .map(|word| word.height)
            .fold(0.0_f32, f32::max)
            .max(1.0);
        let tight_gap = tight_gap_baseline(&words, line_height);
        let mut text = String::new();
        for (index, word) in words.iter().enumerate() {
            let normalized = match normalize_identifier_dash(&words, index) {
                Some(dash) => dash,
                None if index == 0 => match word.text.as_str() {
                    "·" | "●" | "∙" => "•",
                    text => text,
                },
                None => word.text.as_str(),
            };
            if index > 0
                && !text.ends_with(' ')
                && !current_follows_identifier_dash(&words, index)
                && (needs_space_after_ascii_identifier(&words, index, line_height)
                    || needs_space(words[index - 1], word, line_height, tight_gap))
            {
                text.push(' ');
            }
            text.push_str(normalized);
            if index == 0 && normalized == "•" {
                text.push(' ');
            }
        }
        output.push(text.trim_end().to_owned());
    }
    output.join("\r\n")
}

fn repair_curly_quotes(text: &str) -> String {
    if text.contains('“') {
        return text.to_owned();
    }
    let closing_count = text.chars().filter(|ch| *ch == '”').count();
    if closing_count < 2 || !closing_count.is_multiple_of(2) {
        return text.to_owned();
    }
    let mut quote_index = 0_usize;
    text.chars()
        .map(|ch| {
            if ch != '”' {
                return ch;
            }
            let replacement = if quote_index.is_multiple_of(2) {
                '“'
            } else {
                '”'
            };
            quote_index += 1;
            replacement
        })
        .collect()
}

fn model_space_candidate(left: char, right: char) -> bool {
    (is_cjk(left) && right.is_ascii_alphanumeric())
        || (left.is_ascii_alphanumeric() && is_cjk(right))
}

fn valid_character_box(character: &OcrCharacterData) -> bool {
    character.x.is_finite()
        && character.y.is_finite()
        && character.width.is_finite()
        && character.height.is_finite()
        && character.width > 0.0
        && character.height > 0.0
}

fn color_is_foreground(pixel: &[u8; 3], background: &[u8; 3]) -> bool {
    let channel_delta = (0..3)
        .map(|index| pixel[index].abs_diff(background[index]))
        .max()
        .unwrap_or(0);
    let luma = |color: &[u8; 3]| -> i32 {
        (77 * color[0] as i32 + 150 * color[1] as i32 + 29 * color[2] as i32) >> 8
    };
    channel_delta >= 48 || (luma(pixel) - luma(background)).unsigned_abs() >= 32
}

fn local_background(
    image: &RgbImage,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
) -> Option<[u8; 3]> {
    if left >= right || top >= bottom {
        return None;
    }
    let mut bins = [0_u32; 4096];
    for y in top..bottom {
        for x in left..right {
            let pixel = image.get_pixel(x, y).0;
            let index = ((pixel[0] as usize >> 4) << 8)
                | ((pixel[1] as usize >> 4) << 4)
                | (pixel[2] as usize >> 4);
            bins[index] += 1;
        }
    }
    let winning = bins
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| **count)
        .map(|(index, _)| index)?;
    let mut sum = [0_u64; 3];
    let mut count = 0_u64;
    for y in top..bottom {
        for x in left..right {
            let pixel = image.get_pixel(x, y).0;
            let index = ((pixel[0] as usize >> 4) << 8)
                | ((pixel[1] as usize >> 4) << 4)
                | (pixel[2] as usize >> 4);
            if index == winning {
                for channel in 0..3 {
                    sum[channel] += pixel[channel] as u64;
                }
                count += 1;
            }
        }
    }
    (count > 0).then(|| {
        [
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
        ]
    })
}

/// OAR 的字符框来自 CTC 时间步，并不等同于字形外接框。这里只用字符框限定
/// 局部扫描范围，再回到原图寻找实际空白带；证据不足时保持原文。
fn model_boundary_has_visual_space(
    left: &OcrCharacterData,
    right: &OcrCharacterData,
    image: &RgbImage,
) -> bool {
    if !model_space_candidate(left.ch, right.ch)
        || !valid_character_box(left)
        || !valid_character_box(right)
        || image.width() == 0
        || image.height() == 0
    {
        return false;
    }

    let left_center = left.x + left.width * 0.5;
    let right_center = right.x + right.width * 0.5;
    let overlap_top = left.y.max(right.y);
    let overlap_bottom = (left.y + left.height).min(right.y + right.height);
    let minimum_overlap = left.height.min(right.height) * 0.45;
    if !left_center.is_finite()
        || !right_center.is_finite()
        || right_center <= left_center + 2.0
        || overlap_bottom - overlap_top < minimum_overlap
    {
        return false;
    }

    let max_x = image.width() as f32;
    let max_y = image.height() as f32;
    let overlap_height = overlap_bottom - overlap_top;
    // 漏识别空格时，CTC 可能把空白分摊进相邻字符框。向两侧扩少量字高，
    // 在两个字符覆盖的范围内找真正的内部空白带，而不是假定它位于中点。
    let horizontal_margin = overlap_height * 0.20;
    let x_start = (left_center - horizontal_margin).floor().clamp(0.0, max_x) as u32;
    let x_end = (right_center + horizontal_margin).ceil().clamp(0.0, max_x) as u32;
    let inset = overlap_height * 0.08;
    let y_start = (overlap_top + inset).floor().clamp(0.0, max_y) as u32;
    let y_end = (overlap_bottom - inset).ceil().clamp(0.0, max_y) as u32;
    if x_end <= x_start + 2 || y_end <= y_start + 2 {
        return false;
    }
    let Some(background) = local_background(image, x_start, y_start, x_end, y_end) else {
        return false;
    };
    let roi_height = y_end - y_start;
    let minimum_foreground = ((roi_height as f32 * 0.08).ceil() as u32).max(2);
    let column_extent = |x: u32| -> Option<(u32, u32)> {
        let mut first = None;
        let mut last = None;
        let mut count = 0_u32;
        for y in y_start..y_end {
            if color_is_foreground(&image.get_pixel(x, y).0, &background) {
                first.get_or_insert(y);
                last = Some(y);
                count += 1;
            }
        }
        (count >= minimum_foreground).then(|| (first.unwrap_or(y_start), last.unwrap_or(y_start)))
    };

    let mut columns = Vec::with_capacity((x_end - x_start) as usize);
    let mut ink_top = u32::MAX;
    let mut ink_bottom = 0_u32;
    for x in x_start..x_end {
        let extent = column_extent(x);
        if let Some((top, bottom)) = extent {
            ink_top = ink_top.min(top);
            ink_bottom = ink_bottom.max(bottom);
        }
        columns.push(extent);
    }

    if ink_top == u32::MAX {
        return false;
    }
    let ink_height = ink_bottom.saturating_sub(ink_top).saturating_add(1).max(1);
    let required_blank = ((ink_height as f32 * 0.27).ceil() as u32).max(3);

    // CTC 字符框可能把真实空格分摊到任一相邻字符。以 CJK 字符朝向边界的
    // 最外侧墨迹为锚点，只检查向外遇到的第一个、由两侧墨迹闭合的空白带。
    // 首个空白不足阈值时立即保持原文，不能继续钻进相邻字形寻找内部空隙。
    let pixel_box = |character: &OcrCharacterData| -> (u32, u32) {
        // 浮点框描述像素中心；转换成半开整数区间可避免把相邻字符首列算进来。
        let start = (character.x - 0.5)
            .ceil()
            .clamp(x_start as f32, x_end as f32) as u32;
        let end = (character.x + character.width - 0.5)
            .ceil()
            .clamp(x_start as f32, x_end as f32) as u32;
        (start, end)
    };

    if is_cjk(left.ch) {
        let (box_start, box_end) = pixel_box(left);
        let Some(anchor) = (box_start..box_end)
            .rev()
            .find(|x| columns[(x - x_start) as usize].is_some())
        else {
            return false;
        };
        let scan_end = (right.x + right.width * 0.75)
            .ceil()
            .clamp(x_start as f32, x_end as f32) as u32;
        let mut blank_start = None;
        for x in anchor.saturating_add(1)..scan_end {
            if columns[(x - x_start) as usize].is_none() {
                blank_start.get_or_insert(x);
                continue;
            }
            if let Some(start) = blank_start {
                if x.saturating_sub(start) < required_blank || x < box_end {
                    return false;
                }
                let ascii_core_start = right.x + right.width * 0.20;
                let ascii_core_end = right.x + right.width * 0.80;
                for ink_x in x..scan_end {
                    if columns[(ink_x - x_start) as usize].is_none() {
                        break;
                    }
                    let center = ink_x as f32 + 0.5;
                    if center >= ascii_core_start && center < ascii_core_end {
                        return true;
                    }
                }
                return false;
            }
        }
        return false;
    }

    let (box_start, box_end) = pixel_box(right);
    let Some(anchor) = (box_start..box_end).find(|x| columns[(x - x_start) as usize].is_some())
    else {
        return false;
    };
    let scan_start = (left.x + left.width * 0.25)
        .floor()
        .clamp(x_start as f32, x_end as f32) as u32;
    let mut blank_end = None;
    for x in (scan_start..anchor).rev() {
        if columns[(x - x_start) as usize].is_none() {
            blank_end.get_or_insert(x.saturating_add(1));
            continue;
        }
        if let Some(end) = blank_end {
            let start = x.saturating_add(1);
            if end.saturating_sub(start) < required_blank || start > box_start {
                return false;
            }
            let ascii_core_start = left.x + left.width * 0.20;
            let ascii_core_end = left.x + left.width * 0.80;
            for ink_x in (scan_start..=x).rev() {
                if columns[(ink_x - x_start) as usize].is_none() {
                    break;
                }
                let center = ink_x as f32 + 0.5;
                if center >= ascii_core_start && center < ascii_core_end {
                    return true;
                }
            }
            return false;
        }
    }
    false
}

pub(super) fn restore_model_region_spacing(
    text: &str,
    characters: &[OcrCharacterData],
    image: &RgbImage,
) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() != characters.len()
        || chars
            .iter()
            .zip(characters)
            .any(|(ch, character)| *ch != character.ch || !valid_character_box(character))
    {
        return text.to_owned();
    }

    let mut output = String::with_capacity(text.len() + 8);
    for (index, ch) in chars.iter().copied().enumerate() {
        if index > 0
            && !chars[index - 1].is_whitespace()
            && !ch.is_whitespace()
            && model_boundary_has_visual_space(&characters[index - 1], &characters[index], image)
        {
            output.push(' ');
        }
        output.push(ch);
    }
    output
}

fn normalize_model_line(text: &str) -> String {
    let repaired = repair_curly_quotes(text.trim());
    let mut normalized = String::with_capacity(repaired.len() + 8);
    for ch in repaired.chars() {
        if ch.is_whitespace() {
            if !normalized.is_empty() && !normalized.ends_with(' ') {
                normalized.push(' ');
            }
            continue;
        }
        normalized.push(ch);
    }

    let normalized = normalized.trim();
    let Some(first) = normalized.chars().next() else {
        return String::new();
    };
    if matches!(first, '·' | '●' | '∙' | '⚫') {
        let body = normalized[first.len_utf8()..].trim_start();
        return if body.is_empty() {
            String::from("•")
        } else {
            format!("• {body}")
        };
    }
    if first == '•' {
        let body = normalized[first.len_utf8()..].trim_start();
        return if body.is_empty() {
            String::from("•")
        } else {
            format!("• {body}")
        };
    }
    normalized.to_owned()
}

fn same_region_row(group: &[OcrRegionData], region: &OcrRegionData) -> bool {
    let top = group
        .iter()
        .map(|item| item.y)
        .fold(f32::INFINITY, f32::min);
    let bottom = group
        .iter()
        .map(|item| item.y + item.height)
        .fold(f32::NEG_INFINITY, f32::max);
    let center = group
        .iter()
        .map(|item| item.y + item.height * 0.5)
        .sum::<f32>()
        / group.len() as f32;
    let region_center = region.y + region.height * 0.5;
    let overlap = bottom.min(region.y + region.height) - top.max(region.y);
    overlap >= region.height.min(bottom - top).max(1.0) * 0.35
        || (center - region_center).abs() <= region.height.max(bottom - top).max(1.0) * 0.40
}

fn same_indexed_region_row(regions: &[OcrRegionData], group: &[usize], candidate: usize) -> bool {
    let top = group
        .iter()
        .map(|index| regions[*index].y)
        .fold(f32::INFINITY, f32::min);
    let bottom = group
        .iter()
        .map(|index| regions[*index].y + regions[*index].height)
        .fold(f32::NEG_INFINITY, f32::max);
    let center = group
        .iter()
        .map(|index| regions[*index].y + regions[*index].height * 0.5)
        .sum::<f32>()
        / group.len() as f32;
    let region = &regions[candidate];
    let region_center = region.y + region.height * 0.5;
    let overlap = bottom.min(region.y + region.height) - top.max(region.y);
    overlap >= region.height.min(bottom - top).max(1.0) * 0.35
        || (center - region_center).abs() <= region.height.max(bottom - top).max(1.0) * 0.40
}

/// 检测器可能把同一视觉行切成多个区域。跨区域也必须使用首尾字符框和原图
/// 空白作为证据，不能用检测框 padding 或字符类别猜空格。
pub(super) fn restore_model_cross_region_spacing(
    regions: &mut [OcrRegionData],
    characters: &[Vec<OcrCharacterData>],
    image: &RgbImage,
) {
    if regions.len() != characters.len() {
        return;
    }
    let mut indices: Vec<usize> = regions
        .iter()
        .enumerate()
        .filter_map(|(index, region)| {
            (!region.text.trim().is_empty() && region.width > 0.0 && region.height > 0.0)
                .then_some(index)
        })
        .collect();
    indices.sort_by(|left, right| {
        (regions[*left].y + regions[*left].height * 0.5)
            .total_cmp(&(regions[*right].y + regions[*right].height * 0.5))
            .then_with(|| regions[*left].x.total_cmp(&regions[*right].x))
    });
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for index in indices {
        if let Some(row) = rows
            .iter_mut()
            .rev()
            .find(|row| same_indexed_region_row(regions, row, index))
        {
            row.push(index);
        } else {
            rows.push(vec![index]);
        }
    }
    for row in &mut rows {
        row.sort_by(|left, right| regions[*left].x.total_cmp(&regions[*right].x));
        for pair in row.windows(2) {
            let left_index = pair[0];
            let right_index = pair[1];
            let left_region = &regions[left_index];
            let right_region = &regions[right_index];
            if right_region.x - left_region.x - left_region.width
                > left_region.height.max(right_region.height).max(1.0) * 3.0
            {
                continue;
            }
            let left_character = characters[left_index]
                .iter()
                .rev()
                .find(|character| !character.ch.is_whitespace());
            let right_character = characters[right_index]
                .iter()
                .find(|character| !character.ch.is_whitespace());
            if let (Some(left_character), Some(right_character)) = (left_character, right_character)
                && model_boundary_has_visual_space(left_character, right_character, image)
            {
                regions[right_index].space_before = true;
            }
        }
    }
}

/// PP-OCR 检测阶段会把同一视觉行里的多个灰底代码片段分别返回。按坐标合并
/// 同行区域，随后只归一项目符号、成对引号和已由原图验证的空格，不猜正文。
pub(super) fn rebuild_model_ocr_text(regions: &[OcrRegionData]) -> String {
    let mut regions: Vec<OcrRegionData> = regions
        .iter()
        .filter(|region| {
            !region.text.trim().is_empty() && region.width > 0.0 && region.height > 0.0
        })
        .cloned()
        .collect();
    regions.sort_by(|left, right| {
        (left.y + left.height * 0.5)
            .total_cmp(&(right.y + right.height * 0.5))
            .then_with(|| left.x.total_cmp(&right.x))
    });

    let mut rows: Vec<Vec<OcrRegionData>> = Vec::new();
    for region in regions {
        if let Some(row) = rows
            .iter_mut()
            .rev()
            .find(|row| same_region_row(row, &region))
        {
            row.push(region);
        } else {
            rows.push(vec![region]);
        }
    }

    let mut output = Vec::new();
    for mut row in rows {
        row.sort_by(|left, right| left.x.total_cmp(&right.x));
        let mut segments: Vec<Vec<&OcrRegionData>> = vec![Vec::new()];
        for region in &row {
            let starts_new_segment = segments
                .last()
                .and_then(|segment| segment.last().copied())
                .is_some_and(|previous| {
                    region.x - previous.x - previous.width
                        > previous.height.max(region.height).max(1.0) * 3.0
                });
            if starts_new_segment {
                segments.push(Vec::new());
            }
            if let Some(segment) = segments.last_mut() {
                segment.push(region);
            }
        }
        for segment in segments {
            let mut raw = String::new();
            for region in segment {
                if region.space_before && !raw.is_empty() && !raw.ends_with(' ') {
                    raw.push(' ');
                }
                raw.push_str(region.text.trim());
            }
            let text = normalize_model_line(&raw);
            if !text.is_empty() {
                output.push(text);
            }
        }
    }
    output.join("\r\n")
}

pub(super) fn is_cjk_language_tag(tag: &str) -> bool {
    let language = tag
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(language.as_str(), "zh" | "ja" | "ko")
}

fn user_profile_ocr_engine() -> Result<(OcrEngine, bool), String> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|error| format!("无法创建系统 OCR 引擎（请安装对应语言包）：{error}"))?;
    let tag = engine
        .RecognizerLanguage()
        .and_then(|language| language.LanguageTag())
        .map_err(|error| format!("无法读取当前 OCR 语言：{error}"))?
        .to_string();
    Ok((engine, is_cjk_language_tag(&tag)))
}

/// 调用系统自带的 Windows.Media.Ocr。中日韩识别结果按 Windows 返回的行、词和
/// 几何间距重建，保留换行并去掉中文词间伪空格；其他语言保持系统原始排版。
fn recognize_image_text_windows(
    img: &RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Result<String, String> {
    let (engine, rebuild_cjk_layout) = user_profile_ocr_engine()?;
    let max_dimension =
        OcrEngine::MaxImageDimension().map_err(|e| format!("无法读取 OCR 图片尺寸上限：{e}"))?;
    let (rgba, width, height) = prepare_ocr_rgba_for_recognition(img, sel, max_dimension)
        .ok_or_else(|| String::from("选区尺寸无效"))?;

    let writer = DataWriter::new().map_err(|e| format!("无法创建图片缓冲区：{e}"))?;
    writer
        .WriteBytes(&rgba)
        .map_err(|e| format!("无法写入图片缓冲区：{e}"))?;
    let buffer = writer
        .DetachBuffer()
        .map_err(|e| format!("无法读取图片缓冲区：{e}"))?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Rgba8,
        width as i32,
        height as i32,
        BitmapAlphaMode::Straight,
    )
    .map_err(|e| format!("无法创建 OCR 图片：{e}"))?;
    drop(buffer);
    drop(writer);
    drop(rgba);

    let result = engine
        .RecognizeAsync(&bitmap)
        .and_then(|operation| operation.join())
        .map_err(|e| format!("系统 OCR 识别失败：{e}"))?;
    if !rebuild_cjk_layout {
        return result
            .Text()
            .map(|text| text.to_string().trim().to_owned())
            .map_err(|error| format!("无法读取 OCR 结果：{error}"));
    }
    let result_lines = result
        .Lines()
        .map_err(|error| format!("无法读取 OCR 行：{error}"))?;
    let line_count = result_lines
        .Size()
        .map_err(|error| format!("无法读取 OCR 行数：{error}"))?;
    let mut lines = Vec::with_capacity(line_count as usize);
    for line_index in 0..line_count {
        let line = result_lines
            .GetAt(line_index)
            .map_err(|error| format!("无法读取 OCR 行：{error}"))?;
        let result_words = line
            .Words()
            .map_err(|error| format!("无法读取 OCR 词：{error}"))?;
        let word_count = result_words
            .Size()
            .map_err(|error| format!("无法读取 OCR 词数：{error}"))?;
        let mut words = Vec::with_capacity(word_count as usize);
        for word_index in 0..word_count {
            let word = result_words
                .GetAt(word_index)
                .map_err(|error| format!("无法读取 OCR 词：{error}"))?;
            let rect = word
                .BoundingRect()
                .map_err(|error| format!("无法读取 OCR 词位置：{error}"))?;
            words.push(OcrWordData {
                text: word
                    .Text()
                    .map_err(|error| format!("无法读取 OCR 词内容：{error}"))?
                    .to_string(),
                x: rect.X,
                y: rect.Y,
                width: rect.Width,
                height: rect.Height,
            });
        }
        lines.push(OcrLineData { words });
    }
    Ok(rebuild_ocr_text(&regroup_ocr_lines(&lines))
        .trim()
        .to_owned())
}

pub(super) fn choose_ocr_backend(
    model_result: Result<String, String>,
    windows_ocr: impl FnOnce() -> Result<String, String>,
) -> Result<OcrRecognition, String> {
    let (model_error, fallback_reason) = match model_result {
        Ok(text) if !text.trim().is_empty() => {
            return Ok(OcrRecognition {
                text: text.trim().to_owned(),
                backend: OcrBackend::PpOcrV6,
                fallback_reason: None,
            });
        }
        Ok(_) => (
            String::from("高精度 OCR 未识别到文字"),
            OcrFallbackReason::ModelReturnedNoText,
        ),
        Err(error) => (error, OcrFallbackReason::ModelUnavailable),
    };
    windows_ocr()
        .map(|text| OcrRecognition {
            text,
            backend: OcrBackend::Windows,
            fallback_reason: Some(fallback_reason),
        })
        .map_err(|windows_error| {
            format!("高精度 OCR：{model_error}\n系统 OCR 回退：{windows_error}")
        })
}

/// 默认使用随程序发布的 PP-OCRv6 small-det/medium-rec。模型只在一次性子进程中加载，识别
/// 完成后立即释放；子进程不可用或没有结果时回退到 Windows.Media.Ocr。
pub(super) fn recognize_image_text(
    img: &RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Result<OcrRecognition, String> {
    choose_ocr_backend(super::ocr_worker::recognize_with_worker(img, sel), || {
        recognize_image_text_windows(img, sel)
    })
}
