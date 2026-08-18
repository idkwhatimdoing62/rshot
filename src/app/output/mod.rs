mod model;
mod raster;

pub(super) use model::{Annotation, Shape};
pub(super) use raster::render_preview_annotations;

use crate::app::geometry::{crop_image, normalized_rect};
use raster::{TextRasterizer, WindowsTextRasterizer, render_image_annotations};
use std::borrow::Cow;
use xcap::image::RgbaImage;

#[derive(Clone, Copy)]
pub(super) struct OutputDescription<'a> {
    pub(super) frozen_image: &'a RgbaImage,
    pub(super) selection: Option<((i32, i32), (i32, i32))>,
    pub(super) annotations: &'a [Annotation],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OutputFailureStage {
    InvalidSelection,
    TextRasterization,
}

impl OutputFailureStage {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::InvalidSelection => "RSH-OUT-001",
            Self::TextRasterization => "RSH-OUT-002",
        }
    }

    pub(super) const fn description(self) -> &'static str {
        match self {
            Self::InvalidSelection => "选区没有可输出的像素",
            Self::TextRasterization => "文字标注无法完整生成",
        }
    }
}

pub(super) struct ScreenshotOutput<'a> {
    image: Cow<'a, RgbaImage>,
}

impl ScreenshotOutput<'_> {
    pub(super) fn image(&self) -> &RgbaImage {
        self.image.as_ref()
    }

    pub(super) fn dimensions(&self) -> (u32, u32) {
        self.image.dimensions()
    }

    pub(super) fn is_borrowed(&self) -> bool {
        matches!(self.image, Cow::Borrowed(_))
    }

    pub(super) fn into_owned(self) -> RgbaImage {
        self.image.into_owned()
    }
}

pub(super) fn compose(
    description: OutputDescription<'_>,
) -> Result<ScreenshotOutput<'_>, OutputFailureStage> {
    compose_with_text(description, &WindowsTextRasterizer)
}

fn compose_with_text<'a>(
    description: OutputDescription<'a>,
    text: &dyn TextRasterizer,
) -> Result<ScreenshotOutput<'a>, OutputFailureStage> {
    if description.selection.is_none() && description.annotations.is_empty() {
        return Ok(ScreenshotOutput {
            image: Cow::Borrowed(description.frozen_image),
        });
    }
    let mut image = match description.selection {
        Some((a, b)) => crop_image(description.frozen_image, a, b)
            .ok_or(OutputFailureStage::InvalidSelection)?,
        None => description.frozen_image.clone(),
    };
    let origin = description
        .selection
        .map(normalized_rect)
        .map(|rect| (rect.0, rect.1))
        .unwrap_or((0, 0));
    render_image_annotations(&mut image, description.annotations, origin, text)?;
    Ok(ScreenshotOutput {
        image: Cow::Owned(image),
    })
}

#[cfg(test)]
mod tests {
    use super::raster::{TextRaster, TextRasterizer};
    use super::*;
    use xcap::image::Rgba;

    struct FixedText;
    impl TextRasterizer for FixedText {
        fn rasterize(
            &self,
            text: &str,
            _color: [u8; 4],
        ) -> Result<Option<TextRaster>, OutputFailureStage> {
            Ok((!text.is_empty()).then(|| TextRaster {
                width: 1,
                height: 1,
                rgba: vec![255, 0, 0, 255],
            }))
        }
    }

    struct FailedText;
    impl TextRasterizer for FailedText {
        fn rasterize(
            &self,
            _text: &str,
            _color: [u8; 4],
        ) -> Result<Option<TextRaster>, OutputFailureStage> {
            Err(OutputFailureStage::TextRasterization)
        }
    }

    #[test]
    fn unchanged_full_image_is_borrowed() {
        let frozen = RgbaImage::new(2, 2);
        let result = compose(OutputDescription {
            frozen_image: &frozen,
            selection: None,
            annotations: &[],
        })
        .unwrap();
        assert_eq!(result.image().as_ptr(), frozen.as_ptr());
    }

    #[test]
    fn selection_is_normalized_and_annotations_are_translated_and_clipped() {
        let frozen = RgbaImage::from_pixel(5, 5, Rgba([0, 0, 0, 255]));
        let annotations = [Annotation {
            shape: Shape::Line((0, 2), (4, 2)),
            color: [255, 0, 0, 255],
        }];
        let result = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: Some(((4, 4), (1, 1))),
                annotations: &annotations,
            },
            &FixedText,
        )
        .unwrap();
        assert_eq!(result.dimensions(), (3, 3));
        assert_eq!(result.image().get_pixel(0, 1).0, [255, 0, 0, 255]);
        assert_eq!(result.image().get_pixel(2, 1).0, [255, 0, 0, 255]);
    }

    #[test]
    fn rectangle_keeps_a_three_pixel_border_and_transparent_interior() {
        let frozen = RgbaImage::new(20, 20);
        let annotations = [Annotation {
            shape: Shape::Rect((4, 4), (14, 14)),
            color: [255, 0, 0, 255],
        }];
        let output = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: None,
                annotations: &annotations,
            },
            &FixedText,
        )
        .unwrap();

        assert_eq!(output.image().get_pixel(9, 4).0, [255, 0, 0, 255]);
        assert_eq!(output.image().get_pixel(5, 5).0, [255, 0, 0, 255]);
        assert_eq!(output.image().get_pixel(9, 9).0, [0, 0, 0, 0]);
    }

    #[test]
    fn preview_and_output_share_line_pixels() {
        let frozen = RgbaImage::new(8, 8);
        let annotations = [Annotation {
            shape: Shape::Line((1, 1), (6, 6)),
            color: [0, 200, 0, 255],
        }];
        let output = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: None,
                annotations: &annotations,
            },
            &FixedText,
        )
        .unwrap();
        let mut preview = vec![0; 64];
        render_preview_annotations(&mut preview, 8, 8, &annotations);

        for y in 0..8 {
            for x in 0..8 {
                let rgba = output.image().get_pixel(x, y).0;
                let rgb = (rgba[0] as u32) << 16 | (rgba[1] as u32) << 8 | rgba[2] as u32;
                assert_eq!(preview[(y * 8 + x) as usize], rgb);
            }
        }
    }

    #[test]
    fn invalid_selection_is_rejected() {
        let frozen = RgbaImage::new(2, 2);
        let error = compose(OutputDescription {
            frozen_image: &frozen,
            selection: Some(((1, 1), (1, 1))),
            annotations: &[],
        })
        .err();
        assert_eq!(error, Some(OutputFailureStage::InvalidSelection));
    }

    #[test]
    fn text_failure_rejects_the_complete_output() {
        let frozen = RgbaImage::new(2, 2);
        let annotations = [Annotation {
            shape: Shape::Text((0, 0), "x".into()),
            color: [255; 4],
        }];
        let error = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: None,
                annotations: &annotations,
            },
            &FailedText,
        )
        .err();
        assert_eq!(error, Some(OutputFailureStage::TextRasterization));
    }

    #[test]
    fn failure_codes_are_stable() {
        assert_eq!(OutputFailureStage::InvalidSelection.code(), "RSH-OUT-001");
        assert_eq!(OutputFailureStage::TextRasterization.code(), "RSH-OUT-002");
    }
}
