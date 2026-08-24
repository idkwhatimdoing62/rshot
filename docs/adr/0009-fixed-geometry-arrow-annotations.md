# 0009: Fixed-geometry arrow annotations

Status: Accepted

## Context

Users need a directional annotation that remains identical in the editor preview, copied image, and pinned image. Arrow geometry must be deterministic in the physical-pixel coordinate system and must not add a configuration panel before there is evidence that adjustable styling is needed.

## Decision

An arrow annotation stores a start point, end point, and the selected annotation color. The end point is the arrow tip. Rasterization draws a 3 px shaft plus two 12 px arrowhead wings rotated 30 degrees from the reverse shaft direction. Coordinates are rounded to the nearest physical pixel.

A zero-length gesture creates no annotation. Arrows are ordinary annotations: they render after every mosaic, follow creation order and selection clipping, participate in the existing newest-first Undo behavior, and do not affect OCR. Preview and `ScreenshotOutput` call the same arrow rasterization implementation.

## Consequences

The editor interface only needs a new `Arrow(start, end)` shape; callers do not calculate arrowhead points. Arrowheads keep a consistent visible size at every shaft length and display scale. Very short arrows can have wings longer than the shaft, which is accepted for the first version.

## Alternatives

- Scale the arrowhead with shaft length: avoids oversized heads on short arrows but makes appearance less predictable.
- Fill a triangular arrowhead: stronger visual weight but requires polygon rasterization and a separate clipping path.
- Add adjustable width and head size: more control with additional UI and state before evidence of need.
- Implement preview and output separately: simpler local changes but risks copied pixels diverging from the visible arrow.

## Reconsider when

Users need filled heads, adjustable styling, curved arrows, or usability evidence shows fixed heads are unclear on short arrows.

Trace: [Issue #19](https://github.com/idkwhatimdoing62/rshot/issues/19)
