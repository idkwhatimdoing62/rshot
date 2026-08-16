# Issue tracker: GitHub

Issues and specs for this repo live as GitHub issues. Use the `gh` CLI for all operations.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`
- **Read an issue**: `gh issue view <number> --comments`
- **List issues**: use `gh issue list` with suitable JSON fields, labels and state filters
- **Comment**: `gh issue comment <number> --body "..."`
- **Apply/remove labels**: `gh issue edit <number> --add-label "..."` or `--remove-label "..."`
- **Close**: `gh issue close <number> --comment "..."`

Infer the repository from `git remote -v`.

## Pull requests as a triage surface

**PRs as a request surface: no.**

GitHub Issues are the canonical request and triage surface. External pull requests are not automatically placed in the issue triage queue.

## Publishing and fetching

When a skill says “publish to the issue tracker”, create a GitHub issue.

When a skill says “fetch the relevant ticket”, run `gh issue view <number> --comments`.

## Wayfinding operations

A wayfinding map is a GitHub issue labelled `wayfinder:map`. Child work is represented by GitHub sub-issues when available, falling back to task-list links.

Use native issue dependencies for blocking relationships when available. Claim work by assigning the issue to the current user. Resolve it by commenting with the result and closing the issue.
