# 0004: Independent windows for pinned images

Status: Accepted

## Context

Pinned images need OS-level topmost behavior, independent movement and failure isolation. A single composite host would require internal layout, hit testing and z-order management.

## Decision

Each pin owns an RGBA image, Window, Surface and interaction state. `PinCollection` routes by `WindowId`, limits the collection to eight pins and removes only the pin that fails.

## Consequences

Movement and failure isolation are simple. Each pin retains rendering resources, so a fixed capacity prevents unbounded growth.

## Alternatives

- One composite host window: shared resources, substantially more internal window management.

## Reconsider when

Eight pins is routinely insufficient or measured resource use requires shared rendering infrastructure.
