# 0008: Frozen-source mosaic annotations

Status: Accepted

## Context

Users need to obscure local screenshot regions before copying or creating a pinned image. Preview and final output must not diverge, overlapping regions must not compound already pixelated colors, and OCR must continue to receive the immutable source pixels defined by ADR-0001.

## Decision

A mosaic annotation is a normalized rectangle in the physical-pixel coordinate system. Its grid starts at the rectangle's top-left and uses fixed 12×12 pixel blocks. Each block is filled with the average RGBA color sampled from the immutable frozen image.

All mosaic regions are applied before ordinary annotations. Overlapping regions are processed in stable coordinate order and each samples the frozen image independently. The screenshot selection clips the visible result without realigning the grid. Preview and `ScreenshotOutput` use the same rules; OCR ignores every annotation.

## Consequences

Preview, copy and pin produce consistent obscured pixels, while later pen, line, rectangle and text annotations remain legible above mosaic regions. Composing an output with mosaic annotations requires an owned full-size intermediate image before selection cropping, and live preview recomputes affected blocks when the annotation revision changes.

## Alternatives

- Blur the region: smoother appearance, less predictable concealment and different implementation requirements.
- Sample the already composed image: simpler sequential drawing, but overlaps compound and creation order changes the result.
- Align blocks after cropping: less intermediate work, but preview and output can disagree when the selection origin changes.
- Make block size configurable immediately: more control, additional UI and state before evidence of need.

## Reconsider when

Users need adjustable strength, non-rectangular concealment, GPU rendering, or measured preview latency shows that fixed CPU block averaging is not responsive on supported display sizes.

Trace: [Issue #17](https://github.com/idkwhatimdoing62/rshot/issues/17)
