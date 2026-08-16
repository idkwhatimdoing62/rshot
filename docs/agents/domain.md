# Domain Docs

How engineering skills should consume this repository's domain documentation.

## Before exploring, read these

- `CONTEXT.md` at the repository root.
- `docs/adr/` entries relevant to the area being changed.

If these files do not exist, proceed silently. The `domain-modeling` skill creates them lazily when terminology or decisions are resolved.

## File structure

This is a single-context repository:

```text
/
├── CONTEXT.md
├── docs/adr/
└── src/
```

## Use the glossary's vocabulary

When naming a domain concept in issues, refactoring proposals, hypotheses or tests, use the term defined in `CONTEXT.md`. Avoid synonyms that the glossary explicitly rejects.

If a required concept is absent, reconsider whether the term belongs to the project or record the gap for `domain-modeling`.

## Flag ADR conflicts

If proposed work contradicts an existing ADR, identify the conflict explicitly instead of silently overriding the decision.
