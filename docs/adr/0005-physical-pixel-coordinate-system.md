# 0005: Physical pixels as the internal coordinate system

Status: Accepted

## Context

xcap supplies physical pixels while Windows UI frameworks can expose logical coordinates under DPI scaling. Mixing them causes offset, crop and diagonal corruption, especially with mixed-DPI displays and negative origins.

## Decision

rshot requests Per-Monitor V2 awareness and uses physical pixels for monitor matching, window locking, selection, preview, OCR and final output. Conversion stays at platform seams.

## Consequences

One coordinate model maps directly to captured pixels. Monitor matching and DPI initialization must be correct before any window or capture operation begins.

## Alternatives

- Internal logical coordinates: convenient for UI layout, repeated conversion around pixel operations.

## Reconsider when

Cross-platform UI, vector output or cross-display selection requires a richer coordinate model.
