## Issue-First Branch Workflow

### WHAT

- Never make changes directly on `main`.
- Before starting work, create a GitHub Issue that describes the work.
- Perform the work on a non-`main` branch associated with that Issue.

## Documentation

### HOW

- Update related documentation when code changes affect users
- Document usage for new features in README
- Update relevant docs when interfaces change
- Split large docs into separate files in `docs/` folder
- Add links to split docs in README

## File Operations

### HOW

```bash
# File operations
git mv <old-path> <new-path>  # Move files
git rm <path>                  # Delete files
```

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues. See `docs/agents/issue-tracker.md`.

### Triage labels

The default five-role vocabulary is used. See `docs/agents/triage-labels.md`.

### Domain docs

This repository uses a single-context layout. See `docs/agents/domain.md`.

## Tauri Code Organization Rules

### WHY

Keep the generated desktop app split between a small TypeScript frontend and a focused Rust application shell.

### WHAT

- Keep frontend sources in `src/`
- Keep Tauri and Rust sources in `src-tauri/`
- Keep reusable TypeScript logic in `src/lib/`
- Keep generated frontend output in `dist/`
- Keep generated Rust output in `src-tauri/target/`
- Do not edit generated output directories manually
- Never relax lint, TypeScript, rustfmt, or Clippy settings to fix a local issue

### HOW

- Put Tauri command payload shaping in TypeScript helper functions when it needs tests
- Keep Tauri commands small and covered by Rust tests when they contain domain logic
- Use `npm run tauri dev` for desktop development
- Use `npm run check` before handing off changes

## Tauri Testing Guidelines

### WHAT

- **Frontend framework**: Use Vitest for TypeScript unit tests
- **Rust framework**: Use Cargo's built-in test harness for Tauri commands and Rust logic
- **Strategy**: Test observable behavior, not private implementation details
- **Scope**: Keep browser/Tauri runtime calls at entrypoint boundaries and test reusable logic separately

### HOW

- Place TypeScript tests in `tests/` and mirror `src/` structure when practical
- Place Rust unit tests next to the code they cover
- Run the full Tauri quality gate before handing off changes

## Tauri Quality Check

### HOW

```bash
npm run check
```
