# 0010: Editable annotations and bounded history

Status: Accepted

## Context

Annotations were append-only, so correcting a misplaced shape required removing newer work and drawing it again. Users need object-level correction without allowing editor-only selection decoration to leak into copied or pinned pixels.

## Decision

`EditorState` owns one selected annotation and bounded Undo/Redo history. Visual hit testing chooses the topmost ordinary annotation before mosaics, matching output layering. Every completed create, move, resize, recolor, text edit, or delete gesture records one before/after snapshot. History keeps at most 100 entries; a new edit clears Redo.

The Select tool uses `V`. All annotation types move as a whole. Line and arrow endpoints are adjustable; rectangle and mosaic corners resize their normalized bounds; pen strokes only move; text moves and reopens for editing on double-click. `Delete` removes the selected object. `Esc` clears selection before canceling the screenshot session.

Selection outlines and handles are rendered only by the interactive capture frame. `ScreenshotOutput` continues to consume only the frozen image, selection, and annotations, so copy and pin never include editor decoration and OCR continues to read original pixels.

## Consequences

Editing mistakes can be corrected without recreating later annotations, and preview/output consistency remains owned by the existing output boundary. Snapshot history is deliberately simple and bounded; it uses more memory than command deltas but avoids shape-specific inverse operations. Layer reordering and persistent history remain out of scope.

Tracked by GitHub Issue #27.
