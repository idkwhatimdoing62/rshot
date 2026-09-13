## Repository guidance

- Track product issues and durable specs in GitHub Issues. Use `gh` and infer the repository from `git remote -v` when an issue is needed; GitHub Issues, not pull requests, are the canonical triage surface. Do not block small fixes, tests, documentation, or mechanical refactors solely because no issue exists or `gh` is unavailable.
- Use these labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, and `wontfix`.
- Before changing domain behavior, read `docs/CONTEXT.md` and relevant `docs/adr/` entries when present. Use the glossary vocabulary and flag conflicts with recorded decisions.
- For behavior, architecture, or release changes, follow `docs/WORKFLOW.md`: associate and keep a linked GitHub Issue current when one exists, or create/associate one when the change requires product decisions, lasting architectural tradeoffs, or release tracking. Satisfy its closure checklist before declaring that issue complete. If issue tracking is unavailable, continue authorized local work and report the follow-up explicitly.
