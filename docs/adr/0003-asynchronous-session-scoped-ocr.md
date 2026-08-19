# 0003: Asynchronous session-scoped OCR

Status: Accepted

Traceability: [Issue #10](https://github.com/idkwhatimdoing62/rshot/issues/10), [PR #9](https://github.com/idkwhatimdoing62/rshot/pull/9)

## Context

The model worker was isolated in a child process, but the event thread synchronously waited for model recognition and Windows OCR fallback. Pins and other window events could freeze for more than twenty seconds, and a late result could affect a newer screenshot without an identity protocol.

## Decision

`OcrOperation` owns a pixel copy, deadline, cancellation flag and result channel. Every operation has an `OcrSessionId`; completion, failure, timeout and cancellation carry that ID. `App` accepts only the active ID. Model worker wait is bounded to 20 seconds, Windows OCR fallback to 8 seconds, and the complete operation to 30 seconds.

## Consequences

The event loop and pins remain responsive, and cancelled or stale results have no side effects. OCR copies its source pixels. Windows OCR may continue briefly in an isolated thread after the receiver times out because the current adapter cannot force-cancel the WinRT call.

## Alternatives

- Wait synchronously on the event thread: simpler ownership, unacceptable UI freezing.
- Run every backend in a child process: hard termination, more packaging and protocol complexity.

## Reconsider when

Timed-out Windows OCR threads accumulate measurably, concurrent OCR becomes necessary, or the pixel copy becomes a material cost.
