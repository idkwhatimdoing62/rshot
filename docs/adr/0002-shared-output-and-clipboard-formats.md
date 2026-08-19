# 0002: Shared screenshot output and dual image clipboard formats

Status: Accepted

## Context

Copy and pin must crop and render annotations identically. Windows consumers differ: some read bitmap data while Explorer and upload controls commonly consume file drops.

## Decision

Copy and pin consume the same `ScreenshotOutput`. Image clipboard publication attempts both `CF_DIB` and `CF_HDROP`; the latter references a protected managed PNG. Publication reports the formats that actually succeeded and the transaction certainty.

## Consequences

Output semantics stay consistent and more consumers work, at the cost of partial-success handling and a managed temporary-artifact lifecycle.

## Alternatives

- Separate copy and pin rendering: independent evolution, duplicated semantics.
- Publish one clipboard format: simpler ownership, lower compatibility.

## Reconsider when

The two consumers require different color or encoding semantics, or evidence shows another clipboard format is needed.
