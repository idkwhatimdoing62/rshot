## Repository guidance

- This file is a pointer, not a second copy. The change process (issue lifecycle, labels, closure rules) lives in `docs/WORKFLOW.md`; release gates live in `docs/RELEASE.md`. Do not restate their specifics here.
- Before changing domain behavior, read `docs/CONTEXT.md` and relevant `docs/adr/` entries when present. Use the glossary vocabulary and flag conflicts with recorded decisions.
- Per-change verification: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. The heavier gate (`cargo build --release` plus `scripts/run-release-smoke.ps1`) belongs to release-candidate preparation, per `docs/RELEASE.md`. CI runs the full set on `main` and pull requests. Never relax a lint or delete a test to make a gate pass; changing a gate is itself an ADR-worthy decision.
- Documentation, tests and mechanical refactors may proceed without an issue. Behavior changes, lasting architectural tradeoffs and releases need one — `gh` availability is not a reason to skip it, nor a reason to block local work; when issue tracking is unavailable, continue the authorized change and report the follow-up explicitly.
