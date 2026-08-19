## Repository guidance

- Track issues and specs in GitHub Issues. Use `gh` and infer the repository from `git remote -v`; GitHub Issues, not pull requests, are the canonical triage surface.
- Use these labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, and `wontfix`.
- Before changing domain behavior, read `docs/CONTEXT.md` and relevant `docs/adr/` entries when present. Use the glossary vocabulary and flag conflicts with recorded decisions.
- For behavior, architecture, or release changes, follow `docs/WORKFLOW.md`: keep the linked GitHub Issue current and satisfy its closure checklist before declaring completion.
