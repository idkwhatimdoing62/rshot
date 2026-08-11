use super::geometry::normalized_rect;
use std::borrow::Cow;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use xcap::image::{RgbaImage, imageops};

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

/// 调用系统自带的 Windows.Media.Ocr，按用户系统语言识别，不需要联网或随程序携带模型。
pub(super) fn recognize_image_text(
    img: &RgbaImage,
    sel: Option<((i32, i32), (i32, i32))>,
) -> Result<String, String> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("无法创建系统 OCR 引擎（请确认已安装对应语言包）：{e}"))?;
    let max_dimension =
        OcrEngine::MaxImageDimension().map_err(|e| format!("无法读取 OCR 图片尺寸上限：{e}"))?;
    let (rgba, width, height) =
        prepare_ocr_rgba(img, sel, max_dimension).ok_or_else(|| String::from("选区尺寸无效"))?;

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
    let text = result
        .Text()
        .map_err(|e| format!("无法读取 OCR 结果：{e}"))?
        .to_string();
    Ok(text.trim().to_owned())
}
