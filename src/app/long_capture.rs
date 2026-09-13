use xcap::image::{Rgba, RgbaImage};

pub(super) const MAX_LONG_CAPTURE_FRAMES: usize = 40;
pub(super) const MAX_LONG_CAPTURE_HEIGHT: u32 = 32_768;

/// Minimal manual-scroll stitcher. Frames must have the same width.
pub(super) struct LongCapture {
    image: RgbaImage,
    frames: usize,
}

impl LongCapture {
    pub(super) fn start(frame: RgbaImage) -> Result<Self, String> {
        validate_frame(&frame)?;
        Ok(Self {
            image: frame,
            frames: 1,
        })
    }

    pub(super) fn append(&mut self, frame: &RgbaImage) -> Result<(), String> {
        validate_frame(frame)?;
        if self.frames >= MAX_LONG_CAPTURE_FRAMES {
            return Err(String::from("长截图已达到最大段数"));
        }
        if frame.width() != self.image.width() {
            return Err(String::from("长截图各段宽度必须一致"));
        }
        let overlap = best_overlap(&self.image, frame);
        let added = frame.height().saturating_sub(overlap);
        let new_height = self.image.height().saturating_add(added);
        if new_height > MAX_LONG_CAPTURE_HEIGHT {
            return Err(String::from("长截图已达到最大高度"));
        }
        let mut combined = RgbaImage::new(self.image.width(), new_height);
        copy_into(&self.image, &mut combined, 0);
        copy_into(
            frame,
            &mut combined,
            self.image.height().saturating_sub(overlap),
        );
        self.image = combined;
        self.frames += 1;
        Ok(())
    }

    pub(super) fn finish(self) -> RgbaImage {
        self.image
    }
}

fn validate_frame(frame: &RgbaImage) -> Result<(), String> {
    (frame.width() > 0 && frame.height() > 0)
        .then_some(())
        .ok_or_else(|| String::from("长截图帧不能为空"))
}

fn copy_into(source: &RgbaImage, target: &mut RgbaImage, top: u32) {
    for (x, y, pixel) in source.enumerate_pixels() {
        target.put_pixel(x, top + y, *pixel);
    }
}

fn best_overlap(previous: &RgbaImage, next: &RgbaImage) -> u32 {
    let max_overlap = previous.height().min(next.height());
    let min_overlap = 1.min(max_overlap);
    let mut best = 0;
    let mut best_score = u64::MAX;
    for overlap in min_overlap..=max_overlap {
        let score = row_difference(previous, next, overlap);
        if score < best_score {
            best_score = score;
            best = overlap;
        }
    }
    best
}

fn row_difference(previous: &RgbaImage, next: &RgbaImage, overlap: u32) -> u64 {
    let mut score = 0_u64;
    for y in 0..overlap {
        let previous_y = previous.height() - overlap + y;
        for x in 0..previous.width() {
            let a = previous.get_pixel(x, previous_y).0;
            let b = next.get_pixel(x, y).0;
            score += channel_difference(a, b);
        }
    }
    score
}

fn channel_difference(a: [u8; 4], b: [u8; 4]) -> u64 {
    (0..3).map(|index| a[index].abs_diff(b[index]) as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(value: u8) -> Rgba<u8> {
        Rgba([value, value, value, 255])
    }

    #[test]
    fn appends_frames_without_duplicate_overlap() {
        let mut first = RgbaImage::new(2, 3);
        for y in 0..3 {
            for x in 0..2 {
                first.put_pixel(x, y, row(y as u8));
            }
        }
        let mut second = RgbaImage::new(2, 3);
        for y in 0..3 {
            for x in 0..2 {
                second.put_pixel(x, y, row((y + 2) as u8));
            }
        }
        let mut capture = LongCapture::start(first).unwrap();
        capture.append(&second).unwrap();
        assert_eq!(capture.finish().height(), 5);
    }

    #[test]
    fn rejects_mismatched_width() {
        let mut capture = LongCapture::start(RgbaImage::new(2, 2)).unwrap();
        assert!(capture.append(&RgbaImage::new(3, 2)).is_err());
    }
}
