# Repository guidance

## Additional instructions

Before starting work, read these files relative to the repository root:

- [CONTRIBUTING.md](CONTRIBUTING.md) for contribution and release-classification policy.
- `.codex/project.md`
- `.codex/languages/typescript.md`
- `.codex/languages/rust.md`

Within the platform's instruction hierarchy, repository guidance takes precedence in this order: project > language > root common.

## Work boundaries

- Do not make implementation changes directly on `main`.
- Use a non-`main` branch for implementation changes.
- Do not weaken quality checks or test configuration to make a failing change pass.

## Workflow completion

- Reuse explicit user instructions and deterministic repository rules for routine decisions, and carry the requested or explicitly invoked skill through its completion condition within that scope. Do not add a confirmation checkpoint before authorized preparation, commits, push, or PR creation. Preserve the skill's verification requirements.
- Select the workflow from the original task and its delegated scope. Implementation or Issue work includes PR preparation and review completion. Preserve dedicated workflows and their contracts, including consumer Copier updates; template source changes remain ordinary implementation work.
- If a named skill is absent from the catalog, read its `SKILL.md` from the supplied path, checkout `.agents/skills/`, or known personal skill source and resolve its references. Continue when readable; report a concrete missing resource if it is unavailable. Catalog omission alone is not a blocker.
- Ask only when a material implementation decision remains unresolved or the next action lacks authorization. Unapproved external communication or third-party agent requests, merge/deploy/actual release, credentials, permissions and branch protection remain separate authorization boundaries. Honor actual tool or approval denials; repository guidance does not override platform policy.

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
