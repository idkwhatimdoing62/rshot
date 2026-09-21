# 0012: Environment-gated display scenarios record N/A

Status: Accepted

## Context

`C-04` (two displays at equal scale) and `C-05` (mixed scale with negative
desktop coordinates) were recorded BLOCKED in every result from v0.3.0-rc.1
through v0.3.0-rc.4. The cause never changed: the verifying machine has one
display, so no environment it can record represents either topology, and the
evidence rule forbids a single-display snapshot from closing those rows.

BLOCKED cannot express that distinction. It shares one state with unfinished
work, so Issue #1 accumulated silently across four candidates while real
progress inside it stayed invisible: rc.1 blocked C-01 through C-06, P-01
through P-04, O-01 through O-03 and B-01 through B-05, while rc.3 and rc.4 had
narrowed that to only these two hardware-dependent rows. A backlog that no
available action can clear stops being a signal.

## Decision

Mark `C-04` and `C-05` as environment-gated in the matrix. When the verifying
machine cannot provide the required display topology, record `N/A` instead of
`BLOCKED`. `N/A` does not block stable promotion, but every release note must
state which gated topologies remain unverified, and the release record links
this ADR. BLOCKED keeps its previous meaning of unfinished work.

The demotion is suspended for any change to the physical-pixel coordinate model
of ADR-0005 or to the multi-display selection path: the affected scenario
returns to BLOCKED and must be verified on qualifying hardware before
promotion.

## Consequences

Release records now distinguish "no hardware" from "not done", and Issue #1 can
close instead of inheriting silently. Mixed-DPI negative-coordinate behaviour
becomes an accepted unverified risk on single-display machines rather than a
permanent gate, which is why the coordinate-model exception above carries the
safety margin.

## Alternatives

- Procure a second display and a mixed-DPI arrangement for every release:
  correct, but it makes release cadence depend on hardware nobody has; remote
  desktops do not offer a negative-coordinate mixed-scale arrangement either.
- Delete `C-04` and `C-05`: drops a known risk surface that covers real code.
- Leave Issue #1 open forever: four candidates showed the debt is never cleared.

## Reconsider when

A qualifying multi-display environment becomes available, or ADR-0005's
coordinate model changes. Then restore both rows to mandatory verification and
supersede this ADR.
