# 0007: Session-scoped visibility for pinned images

Status: Accepted

Traceability: [Issue #16](https://github.com/idkwhatimdoing62/rshot/issues/16)

## Context

Pins must not enter the frozen image. Restoring them immediately after desktop pixel acquisition kept the frozen pixels correct, but the live always-on-top windows then floated above the selection, editing and OCR experience. It also made capture correctness depend on a narrow Windows compositor timing boundary.

## Decision

`CaptureOperation` acquires one RAII pin-visibility lease before reading desktop pixels. A successful capture transfers that lease into `CaptureSession`; failed preparation drops it locally. Existing pins and pins committed during the operation remain hidden until the session ends. Copy, pin conversion, cancellation, failure, replacement capture and process shutdown restore pins by dropping the owning operation or session.

Windows capture exclusion is a best-effort defense in depth while the lease is active. Failure to configure it does not block capture because hidden-window state is the required behavior.

## Consequences

Pins cannot obscure or intercept input over the screenshot overlay, including while session-scoped OCR runs. The pin collection and event loop remain alive, but pin windows are not interactable until the session ends. Visibility restoration has one ownership path instead of separate success and failure branches.

## Alternatives

- Restore pins immediately after pixel acquisition: shorter hidden period, but pins cover the active screenshot UI.
- Permanently exclude pin windows from capture: avoids timing races, but proved incompatible with visible softbuffer pin windows on the tested Windows environment.
- Destroy and recreate pins around capture: avoids hidden-window state, but loses window identity and increases failure surface.

## Reconsider when

Users need to interact with pins during an active screenshot session, or the screenshot UI moves into a host that can enforce z-order without hiding independent pin windows.
