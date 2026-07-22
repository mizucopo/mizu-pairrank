# Domain docs

This repository uses a single-context domain documentation layout.

## Before exploring

Read the following sources when they exist and are relevant to the work:

- `CONTEXT.md` at the repository root
- ADRs under `docs/adr/`

Proceed silently when either source does not exist. Domain-modeling workflows
create them lazily when terminology or architectural decisions need to be
recorded.

## Vocabulary and decisions

- Use terminology defined in `CONTEXT.md` in issue titles, implementation plans,
  test names, and other engineering output.
- Surface any conflict with an existing ADR explicitly instead of silently
  overriding the decision.

Repositories with multiple bounded contexts should replace this document with
their context map and context-specific documentation paths.
