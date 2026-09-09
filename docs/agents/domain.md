# Domain docs

This repository uses a single-context domain documentation layout.

## Before exploring

Read the following sources when they exist and are relevant to the work:

- `CONTEXT.md` at the repository root
- ADRs under `docs/adr/`

Proceed silently when either source does not exist. Routine exploration alone
does not require creating domain documentation.

## Vocabulary and decisions

- When establishing durable terminology or architectural decisions, create or
  update the relevant `CONTEXT.md` or ADR under `docs/adr/`.
- Use terminology defined in `CONTEXT.md` in issue titles, implementation plans,
  test names, and other engineering output.
- Surface any conflict with an existing ADR explicitly instead of silently
  overriding the decision.

Repositories with multiple bounded contexts should replace this document with
their context map and context-specific documentation paths.
