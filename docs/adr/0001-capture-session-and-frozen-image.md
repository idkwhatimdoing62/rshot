# 0001: Atomic capture session with an immutable frozen image

Status: Accepted

## Context

Selection, preview, OCR and final output must refer to the same screen pixels. Capturing again for each consumer can include changed desktop content or rshot pins, while exposing partially prepared windows in `App` leaves failure cleanup ambiguous.

## Decision

A capture attempt reads the desktop once into an immutable frozen image. It prepares every required window and rendering resource before atomically committing a `CaptureSession`. Selection, OCR and output share that image and physical-pixel coordinate space.

## Consequences

Results remain internally consistent and a failed attempt leaves no half-session. One screen-sized RGBA image remains allocated for the session, and preparation needs RAII cleanup.

## Alternatives

- Capture separately for OCR and output: lower retained memory, inconsistent pixels.
- Mutate `App` after each preparation step: simpler local code, distributed rollback.

## Reconsider when

Video, scrolling capture, cross-display selection or a long asynchronous capture pipeline makes one immutable frame insufficient.
