# Repository guidance

## Additional instructions

Before starting work, read these files relative to the repository root:

- `.codex/project.md`
- `.codex/languages/typescript.md`
- `.codex/languages/rust.md`

Within the platform's instruction hierarchy, repository guidance takes precedence in this order: project > language > root common.

## Work boundaries

- Do not make implementation changes directly on `main`.
- Use a non-`main` branch for implementation changes.
- Do not weaken quality checks or test configuration to make a failing change pass.

## Delegation

- When subagent tools are available, delegate independent, bounded tasks if parallel work can save time or improve quality. Give each task a clear scope and completion criterion, continue useful local work, and verify returned results before integrating them.

## Verification

- Run checks appropriate to the change and all required repository quality gates. Broaden or repeat passed checks only when new changes, failures, or unresolved concerns justify it.
- Add tests that verify meaningful behavior or contracts; avoid tests that merely mirror the implementation for reversible, low-impact changes.

## Project context

- For issue creation and management, follow `docs/agents/issue-tracker.md`.
- For issue triage, use the label mapping in `docs/agents/triage-labels.md`.
- Before changing domain terminology or architecture, follow `docs/agents/domain.md` and its referenced context and ADRs.

## Template updates

- Generated configuration and source files remain Copier-managed. For routine updates, use `copier update`, not `copier recopy`.
- Update from `repo-template/main` with `copier update --trust --defaults --vcs-ref HEAD` on a clean non-`main` branch.
- Review the complete diff and resolve every Copier conflict before running the repository quality gate.
