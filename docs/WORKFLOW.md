# Change workflow

This repository separates durable facts from the temporary process used to change them.

## Sources of truth

| Information | Authoritative location |
| --- | --- |
| User-facing entry points | `README.md` |
| Domain vocabulary | `docs/CONTEXT.md` |
| Current system behavior and structure | `docs/DESIGN.md` |
| Accepted architecture decisions and tradeoffs | `docs/adr/` |
| Release policy | `docs/RELEASE.md` |
| Version history | `CHANGELOG.md` |
| Per-release evidence | `docs/release/results/` |
| Current change plan and discoveries | GitHub Issue |
| Exact implementation | Source code, tests and configuration |

Owner Docs describe the accepted current state. GitHub Issues describe how a proposed change moves the system to another state. A completed Issue does not remain an implicit architecture rule: durable conclusions move into the appropriate Owner Doc or ADR.

## Issue lifecycle

1. **Open:** state why the change is needed, its scope, acceptance criteria, known risks and affected Owner Docs. Apply `needs-triage`.
2. **Ready:** resolve material questions and apply `ready-for-agent` or `ready-for-human`. Use `needs-info` while a required decision is missing.
3. **Implement:** link commits or the PR. Keep discoveries, scope changes and intentionally unfinished work in the Issue rather than only in chat.
4. **Reconcile:** compare the implementation with every affected Owner Doc. Update facts that changed. Add an ADR when a choice has meaningful alternatives, lasting consequences or a future reevaluation condition.
5. **Close:** record the delivered result and verification. Move unfinished work to linked Issues. Close only when the checklist below is true; use `wontfix` when the proposal is intentionally rejected.

## Closure checklist

- Acceptance criteria are either verified or explicitly moved to a linked follow-up Issue.
- Required quality commands pass, with interactive gaps linked to a `ready-for-human` Issue.
- `README.md`, `docs/CONTEXT.md`, `docs/DESIGN.md`, `docs/RELEASE.md` and `CHANGELOG.md` were checked where relevant.
- Lasting decisions are recorded in `docs/adr/`; superseded ADRs link to their replacements.
- Discoveries and remaining work are written in the Issue.
- The final Issue comment links the merged change and states what was verified.

## ADR format

Name ADRs `NNNN-short-title.md`. Each ADR records Status, Context, Decision, Consequences, Alternatives and Reconsider when. Accepted ADRs are immutable except for clarification; a changed decision creates a new ADR and marks the old one Superseded.

Do not create repository plan or development-log Markdown files. GitHub Issues are the canonical process record; this keeps temporary execution detail out of Owner Docs.
