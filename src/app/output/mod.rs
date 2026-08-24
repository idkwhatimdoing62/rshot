mod model;
mod raster;

pub(super) use model::{Annotation, Shape};
pub(super) use raster::render_preview_annotations;

use crate::app::geometry::{crop_image, normalized_rect};
use raster::{
    TextRasterizer, WindowsTextRasterizer, apply_image_mosaics, render_image_annotations,
};
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
    let has_mosaic = description
        .annotations
        .iter()
        .any(|annotation| matches!(annotation.shape, Shape::Mosaic(..)));
    let mut mosaic_image = has_mosaic.then(|| description.frozen_image.clone());
    if let Some(image) = mosaic_image.as_mut() {
        apply_image_mosaics(image, description.frozen_image, description.annotations);
    }
    let source = mosaic_image.as_ref().unwrap_or(description.frozen_image);
    let mut image = match description.selection {
        Some((a, b)) => crop_image(source, a, b).ok_or(OutputFailureStage::InvalidSelection)?,
        None => source.clone(),
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
        render_preview_annotations(&mut preview, 8, 8, &frozen, None, &annotations);

        for y in 0..8 {
            for x in 0..8 {
                let rgba = output.image().get_pixel(x, y).0;
                let rgb = (rgba[0] as u32) << 16 | (rgba[1] as u32) << 8 | rgba[2] as u32;
                assert_eq!(preview[(y * 8 + x) as usize], rgb);
            }
        }
    }

    #[test]
    fn preview_and_output_share_fixed_arrow_geometry() {
        let frozen = RgbaImage::new(32, 32);
        let annotations = [Annotation {
            shape: Shape::Arrow((4, 16), (20, 16)),
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
        let mut preview = vec![0; 32 * 32];
        render_preview_annotations(&mut preview, 32, 32, &frozen, None, &annotations);

        for point in [(4, 16), (20, 16), (10, 10), (10, 22)] {
            assert_eq!(
                output.image().get_pixel(point.0, point.1).0,
                [255, 0, 0, 255]
            );
            assert_eq!(preview[(point.1 * 32 + point.0) as usize], 0x00ff0000);
        }
        for y in 0..32 {
            for x in 0..32 {
                let rgba = output.image().get_pixel(x, y).0;
                let rgb = (rgba[0] as u32) << 16 | (rgba[1] as u32) << 8 | rgba[2] as u32;
                assert_eq!(preview[(y * 32 + x) as usize], rgb);
            }
        }
    }

    #[test]
    fn mosaic_block_uses_the_average_color_from_frozen_pixels() {
        let frozen = RgbaImage::from_fn(4, 2, |x, y| Rgba([((y * 4 + x) * 10) as u8, 0, 0, 255]));
        let annotations = [Annotation {
            shape: Shape::Mosaic((0, 0), (4, 2)),
            color: [0; 4],
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

        assert!(
            output
                .image()
                .pixels()
                .all(|pixel| pixel.0 == [35, 0, 0, 255])
        );
    }

    #[test]
    fn preview_and_output_share_mosaic_pixels() {
        let frozen = RgbaImage::from_fn(4, 2, |x, y| Rgba([((y * 4 + x) * 10) as u8, 20, 30, 255]));
        let annotations = [Annotation {
            shape: Shape::Mosaic((0, 0), (4, 2)),
            color: [0; 4],
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
        let mut preview = frozen
            .pixels()
            .map(|pixel| {
                (u32::from(pixel[0]) << 16) | (u32::from(pixel[1]) << 8) | u32::from(pixel[2])
            })
            .collect::<Vec<_>>();

        render_preview_annotations(&mut preview, 4, 2, &frozen, None, &annotations);

        for (actual, expected) in preview.iter().zip(output.image().pixels()) {
            assert_eq!(
                *actual,
                (u32::from(expected[0]) << 16)
                    | (u32::from(expected[1]) << 8)
                    | u32::from(expected[2])
            );
        }
    }

    #[test]
    fn selection_crops_mosaic_without_realigning_its_twelve_pixel_grid() {
        let frozen = RgbaImage::from_fn(24, 1, |x, _| Rgba([x as u8, 0, 0, 255]));
        let annotations = [Annotation {
            shape: Shape::Mosaic((0, 0), (24, 1)),
            color: [0; 4],
        }];

        let output = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: Some(((6, 0), (18, 1))),
                annotations: &annotations,
            },
            &FixedText,
        )
        .unwrap();

        assert_eq!(output.dimensions(), (12, 1));
        assert!(output.image().pixels().take(6).all(|pixel| pixel[0] == 5));
        assert!(output.image().pixels().skip(6).all(|pixel| pixel[0] == 17));
    }

    #[test]
    fn preview_clips_mosaic_to_selection_without_realigning_the_grid() {
        let frozen = RgbaImage::from_fn(24, 1, |x, _| Rgba([x as u8, 0, 0, 255]));
        let annotations = [Annotation {
            shape: Shape::Mosaic((0, 0), (24, 1)),
            color: [0; 4],
        }];
        let mut preview = (0_u32..24).map(|x| x << 16).collect::<Vec<_>>();

        render_preview_annotations(
            &mut preview,
            24,
            1,
            &frozen,
            Some(((6, 0), (18, 1))),
            &annotations,
        );

        assert_eq!(preview[0], 0);
        assert!(preview[6..12].iter().all(|pixel| *pixel == 5 << 16));
        assert!(preview[12..18].iter().all(|pixel| *pixel == 17 << 16));
        assert_eq!(preview[23], 23 << 16);
    }

    #[test]
    fn overlapping_mosaics_sample_the_frozen_image_independent_of_creation_order() {
        let frozen = RgbaImage::from_fn(24, 1, |x, _| Rgba([x as u8, 0, 0, 255]));
        let first = Annotation {
            shape: Shape::Mosaic((0, 0), (24, 1)),
            color: [0; 4],
        };
        let second = Annotation {
            shape: Shape::Mosaic((6, 0), (18, 1)),
            color: [0; 4],
        };

        let forward = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: None,
                annotations: &[first.clone(), second.clone()],
            },
            &FixedText,
        )
        .unwrap()
        .into_owned();
        let reversed = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: None,
                annotations: &[second, first],
            },
            &FixedText,
        )
        .unwrap()
        .into_owned();

        assert_eq!(forward, reversed);
    }

    #[test]
    fn ordinary_annotations_are_drawn_above_mosaic_regions() {
        let frozen = RgbaImage::from_fn(24, 3, |x, _| Rgba([x as u8, 0, 0, 255]));
        let annotations = [
            Annotation {
                shape: Shape::Line((0, 1), (23, 1)),
                color: [255, 0, 0, 255],
            },
            Annotation {
                shape: Shape::Mosaic((0, 0), (24, 3)),
                color: [0; 4],
            },
        ];

        let output = compose_with_text(
            OutputDescription {
                frozen_image: &frozen,
                selection: None,
                annotations: &annotations,
            },
            &FixedText,
        )
        .unwrap();

        assert!((0..24).all(|x| output.image().get_pixel(x, 1).0 == [255, 0, 0, 255]));
    }

    #[test]
    fn clipping_at_the_frozen_image_edge_does_not_realign_the_mosaic_grid() {
        let frozen = RgbaImage::from_fn(20, 1, |x, _| Rgba([x as u8, 0, 0, 255]));
        let annotations = [Annotation {
            shape: Shape::Mosaic((-5, 0), (15, 1)),
            color: [0; 4],
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

        assert!(output.image().pixels().take(7).all(|pixel| pixel[0] == 3));
        assert!(
            output
                .image()
                .pixels()
                .skip(7)
                .take(8)
                .all(|pixel| pixel[0] == 10)
        );
        assert_eq!(output.image().get_pixel(15, 0)[0], 15);
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
