# Change workflow

This repository separates durable facts from the temporary process used to change them.

## Sources of truth

| Information | Authoritative location |
| --- | --- |
| User-facing entry points | `README.md` |
| Domain vocabulary | `docs/CONTEXT.md` |
| Current system behavior and structure | `docs/ARCHITECTURE.md` |
| Accepted architecture decisions and tradeoffs | `docs/adr/` |
| Release policy | `docs/RELEASE.md` |
| Version history | `CHANGELOG.md` |
| Per-release evidence | `docs/release/results/` |
| Pre-decision research and teaching notes | `docs/research/`, `docs/notes/` |
| Current change plan and discoveries | GitHub Issue |
| Exact implementation | Source code, tests and configuration |

Owner Docs describe the accepted current state. GitHub Issues describe how a proposed change moves the system to another state. A completed Issue does not remain an implicit architecture rule: durable conclusions move into the appropriate Owner Doc or ADR.

## Issue lifecycle

1. **Open:** state why the change is needed, its scope, acceptance criteria, known risks and affected Owner Docs. An Issue with no label is still under discussion; that is a valid state.
2. **Ready:** resolve material questions, then label `ready-for-agent` (executable without a human decision) or `ready-for-human` (needs interactive Windows verification). These two are the whole taxonomy; an outstanding decision stays written in the Issue body instead of getting its own label.
3. **Implement:** link commits or the PR. Keep discoveries, scope changes and intentionally unfinished work in the Issue rather than only in chat.
4. **Reconcile:** compare the implementation with every affected Owner Doc. Update facts that changed. Add an ADR when a choice has meaningful alternatives, lasting consequences or a future reevaluation condition.
5. **Close:** record the delivered result and verification. Move unfinished work to linked Issues. Close only when the checklist below is true. An intentionally rejected proposal also closes, with the reason in its final comment.

## Closure checklist

- Every acceptance-criteria box is checked, or that item has been moved to a linked follow-up Issue. An Issue may not close while its own description still shows unchecked boxes: a stale `- [ ]` states something false about the delivered result.
- The per-change commands pass and the final Issue comment links the change and states exactly what was verified; interactive gaps get their own `ready-for-human` Issue.
- Reconciled affected Owner Docs (`README.md`, `docs/CONTEXT.md`, `docs/ARCHITECTURE.md`, `docs/RELEASE.md`, `CHANGELOG.md`) and moved durable conclusions into them or into `docs/adr/`, with superseded ADRs linking to their replacements.

## ADR format

Name ADRs `NNNN-short-title.md`. Each ADR records Status, Context, Decision, Consequences, Alternatives and Reconsider when. Accepted ADRs are immutable except for clarification; a changed decision creates a new ADR and marks the old one Superseded.

Do not create repository plan or development-log Markdown files. GitHub Issues are the canonical process record; this keeps temporary execution detail out of Owner Docs.
