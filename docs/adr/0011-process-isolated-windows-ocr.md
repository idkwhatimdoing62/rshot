# 0011: Process-isolated Windows OCR fallback

Status: Accepted

Supersedes: [ADR-0003](0003-asynchronous-session-scoped-ocr.md)

## Context

The Windows OCR fallback previously ran in a detached thread. A WinRT OCR call
could outlive its timeout and retain the image copy and apartment indefinitely.
An in-process thread cannot be safely force-terminated.

## Decision

Run the Windows OCR fallback in the existing self-executable OCR worker process.
The parent sends the same bounded RGBA request, waits at most eight seconds, and
kills and waits for the child and its pipe-reader threads on timeout or failure.
The worker initializes its own WinRT apartment and returns only UTF-8 text.

## Consequences

Timed-out Windows OCR no longer leaves an unkillable thread or WinRT apartment
in the main process. The worker process has a small amount of additional startup
and IPC overhead, and the existing worker job-object boundary must remain intact.

## Alternatives

- Keep a detached thread and suppress concurrent requests: bounds accumulation
  but can permanently disable the fallback when WinRT never returns.
- Wait synchronously on the event thread: simpler but freezes the UI.

## Reconsider when

Windows provides a cancellable OCR operation that can be safely awaited and
terminated in-process, or process startup becomes a measured user-visible cost.
