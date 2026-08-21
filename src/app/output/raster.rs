use super::{Annotation, OutputFailureStage, Shape};
use crate::app::geometry::normalized_rect;
use crate::app::windows_adapter::gdi_render_text_rgba;
use xcap::image::{Rgba, RgbaImage};

pub(super) const ANNOT_LINE_T: i32 = 1;
pub(super) const MOSAIC_BLOCK_SIZE: i32 = 12;

pub(super) trait TextRasterizer {
    fn rasterize(
        &self,
        text: &str,
        color: [u8; 4],
    ) -> Result<Option<TextRaster>, OutputFailureStage>;
}

pub(super) struct WindowsTextRasterizer;

impl TextRasterizer for WindowsTextRasterizer {
    fn rasterize(
        &self,
        text: &str,
        color: [u8; 4],
    ) -> Result<Option<TextRaster>, OutputFailureStage> {
        if text.is_empty() {
            return Ok(None);
        }
        gdi_render_text_rgba(text, color)
            .map(Some)
            .map(|value| {
                value.map(|(width, height, rgba)| TextRaster {
                    width,
                    height,
                    rgba,
                })
            })
            .ok_or(OutputFailureStage::TextRasterization)
    }
}

struct PreviewTextRasterizer;

impl TextRasterizer for PreviewTextRasterizer {
    fn rasterize(
        &self,
        text: &str,
        color: [u8; 4],
    ) -> Result<Option<TextRaster>, OutputFailureStage> {
        Ok(WindowsTextRasterizer.rasterize(text, color).ok().flatten())
    }
}

pub(super) struct TextRaster {
    pub(super) width: i32,
    pub(super) height: i32,
    pub(super) rgba: Vec<u8>,
}

trait RasterTarget {
    fn draw_line(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4], radius: i32);
    fn draw_rect(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4], thickness: i32);
    fn blend(&mut self, position: (i32, i32), raster: &TextRaster);
}

pub(super) fn render_image_annotations(
    image: &mut RgbaImage,
    annotations: &[Annotation],
    origin: (i32, i32),
    text: &dyn TextRasterizer,
) -> Result<(), OutputFailureStage> {
    render_annotations(&mut ImageTarget(image), annotations, origin, text)
}

pub(super) fn apply_image_mosaics(
    target: &mut RgbaImage,
    frozen_image: &RgbaImage,
    annotations: &[Annotation],
) {
    for (a, b) in sorted_mosaic_regions(annotations) {
        pixelate_image(target, frozen_image, a, b);
    }
}

fn sorted_mosaic_regions(annotations: &[Annotation]) -> Vec<((i32, i32), (i32, i32))> {
    let mut regions = annotations
        .iter()
        .filter_map(|annotation| {
            let Shape::Mosaic(a, b) = annotation.shape else {
                return None;
            };
            let (left, top, right, bottom) = normalized_rect((a, b));
            Some(((left, top), (right, bottom)))
        })
        .collect::<Vec<_>>();
    regions.sort_unstable();
    regions
}

fn pixelate_image(target: &mut RgbaImage, source: &RgbaImage, a: (i32, i32), b: (i32, i32)) {
    for_each_mosaic_block(source, a, b, |bounds, average| {
        let (left, top, right, bottom) = bounds;
        let average = Rgba(average);
        for y in top..bottom {
            for x in left..right {
                target.put_pixel(x as u32, y as u32, average);
            }
        }
    });
}

fn for_each_mosaic_block(
    source: &RgbaImage,
    a: (i32, i32),
    b: (i32, i32),
    mut apply: impl FnMut((i32, i32, i32, i32), [u8; 4]),
) {
    let (left, top, right, bottom) = normalized_rect((a, b));
    let mut block_y = top;
    while block_y < bottom {
        let block_bottom = block_y.saturating_add(MOSAIC_BLOCK_SIZE).min(bottom);
        let mut block_x = left;
        while block_x < right {
            let block_right = block_x.saturating_add(MOSAIC_BLOCK_SIZE).min(right);
            let sample_left = block_x.clamp(0, source.width() as i32);
            let sample_top = block_y.clamp(0, source.height() as i32);
            let sample_right = block_right.clamp(0, source.width() as i32);
            let sample_bottom = block_bottom.clamp(0, source.height() as i32);
            let mut sums = [0_u64; 4];
            let mut count = 0_u64;
            for y in sample_top..sample_bottom {
                for x in sample_left..sample_right {
                    let pixel = source.get_pixel(x as u32, y as u32).0;
                    for channel in 0..4 {
                        sums[channel] += u64::from(pixel[channel]);
                    }
                    count += 1;
                }
            }
            if count > 0 {
                let average = sums.map(|sum| (sum / count) as u8);
                apply(
                    (sample_left, sample_top, sample_right, sample_bottom),
                    average,
                );
            }
            block_x = block_right;
        }
        block_y = block_bottom;
    }
}

pub(crate) fn render_preview_annotations(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    frozen_image: &RgbaImage,
    selection: Option<((i32, i32), (i32, i32))>,
    annotations: &[Annotation],
) {
    let clip = selection
        .map(normalized_rect)
        .unwrap_or((0, 0, width as i32, height as i32));
    for (a, b) in sorted_mosaic_regions(annotations) {
        pixelate_buffer(buffer, width, height, frozen_image, a, b, clip);
    }
    let _ = render_annotations(
        &mut BufferTarget {
            buffer,
            width,
            height,
        },
        annotations,
        (0, 0),
        &PreviewTextRasterizer,
    );
}

fn pixelate_buffer(
    target: &mut [u32],
    width: u32,
    height: u32,
    source: &RgbaImage,
    a: (i32, i32),
    b: (i32, i32),
    clip: (i32, i32, i32, i32),
) {
    for_each_mosaic_block(source, a, b, |bounds, average| {
        let color =
            u32::from(average[0]) << 16 | u32::from(average[1]) << 8 | u32::from(average[2]);
        let write_left = bounds.0.max(clip.0);
        let write_top = bounds.1.max(clip.1);
        let write_right = bounds.2.min(clip.2).min(width as i32);
        let write_bottom = bounds.3.min(clip.3).min(height as i32);
        if write_left < write_right && write_top < write_bottom {
            for y in write_top..write_bottom {
                for x in write_left..write_right {
                    target[(y as u32 * width + x as u32) as usize] = color;
                }
            }
        }
    });
}

fn render_annotations(
    target: &mut dyn RasterTarget,
    annotations: &[Annotation],
    origin: (i32, i32),
    text: &dyn TextRasterizer,
) -> Result<(), OutputFailureStage> {
    let translate = |point: (i32, i32)| (point.0 - origin.0, point.1 - origin.1);
    for annotation in annotations {
        match &annotation.shape {
            Shape::Pen(points) => {
                for pair in points.windows(2) {
                    target.draw_line(
                        translate(pair[0]),
                        translate(pair[1]),
                        annotation.color,
                        ANNOT_LINE_T,
                    );
                }
            }
            Shape::Line(a, b) => {
                target.draw_line(translate(*a), translate(*b), annotation.color, ANNOT_LINE_T)
            }
            Shape::Rect(a, b) => {
                target.draw_rect(translate(*a), translate(*b), annotation.color, 3)
            }
            Shape::Mosaic(..) => {}
            Shape::Text(position, value) => {
                if let Some(raster) = text.rasterize(value, annotation.color)? {
                    target.blend(translate(*position), &raster);
                }
            }
        }
    }
    Ok(())
}

struct ImageTarget<'a>(&'a mut RgbaImage);

impl RasterTarget for ImageTarget<'_> {
    fn draw_line(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4], radius: i32) {
        draw_line_image(self.0, a, b, color, radius);
    }

    fn draw_rect(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4], thickness: i32) {
        let (left, top, right, bottom) = normalized_rect((a, b));
        for d in 0..thickness {
            draw_line_image(self.0, (left, top + d), (right, top + d), color, 0);
            draw_line_image(self.0, (left, bottom - d), (right, bottom - d), color, 0);
            draw_line_image(self.0, (left + d, top), (left + d, bottom), color, 0);
            draw_line_image(self.0, (right - d, top), (right - d, bottom), color, 0);
        }
    }

    fn blend(&mut self, position: (i32, i32), raster: &TextRaster) {
        blend_rgba_image(self.0, position, raster);
    }
}

struct BufferTarget<'a> {
    buffer: &'a mut [u32],
    width: u32,
    height: u32,
}

impl RasterTarget for BufferTarget<'_> {
    fn draw_line(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4], radius: i32) {
        draw_line_buffer(
            self.buffer,
            self.width,
            self.height,
            a,
            b,
            color_u32(color),
            radius,
        );
    }

    fn draw_rect(&mut self, a: (i32, i32), b: (i32, i32), color: [u8; 4], thickness: i32) {
        draw_rect_buffer(
            self.buffer,
            self.width,
            self.height,
            a,
            b,
            color_u32(color),
            thickness,
        );
    }

    fn blend(&mut self, position: (i32, i32), raster: &TextRaster) {
        blend_rgba_buffer(self.buffer, self.width, self.height, position, raster);
    }
}

fn draw_line_buffer(
    buffer: &mut [u32],
    width: u32,
    height: u32,
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
                    if px >= 0 && py >= 0 && px < width as i32 && py < height as i32 {
                        buffer[(py as u32 * width + px as u32) as usize] = color;
                    }
                }
            }
        }
    }
}

fn draw_rect_buffer(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    a: (i32, i32),
    b: (i32, i32),
    color: u32,
    thickness: i32,
) {
    if width == 0 || height == 0 {
        return;
    }
    let (left, top, right, bottom) = normalized_rect((a, b));
    let left = left.clamp(0, width as i32 - 1);
    let right = right.clamp(0, width as i32 - 1);
    let top = top.clamp(0, height as i32 - 1);
    let bottom = bottom.clamp(0, height as i32 - 1);
    for d in 0..thickness.max(1) {
        let yt = (top + d).min(height as i32 - 1);
        let yb = (bottom - d).max(0);
        for x in left..=right {
            buffer[(yt as u32 * width + x as u32) as usize] = color;
            buffer[(yb as u32 * width + x as u32) as usize] = color;
        }
        let xl = (left + d).min(width as i32 - 1);
        let xr = (right - d).max(0);
        for y in top..=bottom {
            buffer[(y as u32 * width + xl as u32) as usize] = color;
            buffer[(y as u32 * width + xr as u32) as usize] = color;
        }
    }
}

fn draw_line_image(
    image: &mut RgbaImage,
    a: (i32, i32),
    b: (i32, i32),
    color: [u8; 4],
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
                let (px, py) = (x + ox, y + oy);
                if ox * ox + oy * oy <= radius * radius
                    && px >= 0
                    && py >= 0
                    && px < image.width() as i32
                    && py < image.height() as i32
                {
                    image.put_pixel(px as u32, py as u32, Rgba(color));
                }
            }
        }
    }
}

fn blend_rgba_buffer(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    position: (i32, i32),
    raster: &TextRaster,
) {
    for y in 0..raster.height {
        for x in 0..raster.width {
            let index = ((y * raster.width + x) * 4) as usize;
            let alpha = raster.rgba[index + 3] as u32;
            if alpha == 0 {
                continue;
            }
            let (px, py) = (position.0 + x, position.1 + y);
            if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                continue;
            }
            let destination = buffer[(py as u32 * width + px as u32) as usize];
            let inverse = 255 - alpha;
            let red =
                (raster.rgba[index] as u32 * alpha + ((destination >> 16) & 0xff) * inverse) / 255;
            let green = (raster.rgba[index + 1] as u32 * alpha
                + ((destination >> 8) & 0xff) * inverse)
                / 255;
            let blue =
                (raster.rgba[index + 2] as u32 * alpha + (destination & 0xff) * inverse) / 255;
            buffer[(py as u32 * width + px as u32) as usize] = (red << 16) | (green << 8) | blue;
        }
    }
}

fn blend_rgba_image(image: &mut RgbaImage, position: (i32, i32), raster: &TextRaster) {
    for y in 0..raster.height {
        for x in 0..raster.width {
            let index = ((y * raster.width + x) * 4) as usize;
            let alpha = raster.rgba[index + 3] as u32;
            if alpha == 0 {
                continue;
            }
            let (px, py) = (position.0 + x, position.1 + y);
            if px < 0 || py < 0 || px >= image.width() as i32 || py >= image.height() as i32 {
                continue;
            }
            let destination = image.get_pixel(px as u32, py as u32).0;
            let inverse = 255 - alpha;
            let red = (raster.rgba[index] as u32 * alpha + destination[0] as u32 * inverse) / 255;
            let green =
                (raster.rgba[index + 1] as u32 * alpha + destination[1] as u32 * inverse) / 255;
            let blue =
                (raster.rgba[index + 2] as u32 * alpha + destination[2] as u32 * inverse) / 255;
            image.put_pixel(
                px as u32,
                py as u32,
                Rgba([red as u8, green as u8, blue as u8, 255]),
            );
        }
    }
}

pub(super) fn color_u32(color: [u8; 4]) -> u32 {
    (color[0] as u32) << 16 | (color[1] as u32) << 8 | color[2] as u32
}
