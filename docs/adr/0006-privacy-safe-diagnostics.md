# 0006: Privacy-safe diagnostics with stable error codes

Status: Accepted

## Context

Screenshots, OCR text, window titles, coordinates and paths can contain private information. Raw system errors aid local debugging but are unsafe as a default artifact for public issue reports.

## Decision

Persistent diagnostics contain only time, package version, event and stable `RSH-*` code. Export reparses the log and admits only whitelisted fields. Raw errors may appear in the current local prompt but are not persisted.

## Consequences

Users can attach diagnostics with a narrow privacy boundary. Remote diagnosis has less context and may require a safe reproduction.

## Alternatives

- Persist raw errors and environment details: richer diagnosis, unacceptable default disclosure risk.

## Reconsider when

An explicitly authorized local advanced-diagnostics mode can provide stronger evidence without weakening the default boundary.
