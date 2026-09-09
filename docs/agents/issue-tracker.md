# Issue tracker: GitHub Issues

Issues for this repository are tracked in GitHub Issues. Infer the repository
from the configured Git remote instead of hard-coding an owner or repository
name in agent instructions.

## Resolve the target repository

Before creating an Issue, including a review-follow-up Issue:

1. Use an explicit target repository from the user's request when provided.
   Otherwise, inspect `git remote -v` and available GitHub repository metadata,
   including fork relationships. Treat remote names such as `origin` and
   `upstream` as hints, not proof of the Issue destination.
2. Select a remote's repository when it is the single unambiguous non-fork
   repository and the task context agrees. Fetch and push URLs that identify
   the same repository count as one candidate.
3. For a fork or multiple distinct remote repositories, use the source Issue,
   the reviewed PR's base repository, and repository contribution guidance to
   establish where the Issue belongs. A fork parent or a push destination
   alone does not establish that target.
4. If the target remains unclear, including when no remote is configured or
   metadata is unavailable, ask the user for the target repository URL. Show
   any candidates and the ambiguity. Wait for an answer before creating the
   Issue; unrelated read-only investigation may continue.
5. Verify the selected repository is accessible and accepts Issues, then pass
   that repository explicitly to the GitHub integration or `gh --repo`.
   Preserve its GitHub host as well as owner and name. If verification fails,
   report the blocker and confirm the intended target rather than falling
   back to another repository.

## Conventions

- Create review-follow-up Issues as GitHub Issues.
- Read the full issue body, comments, and labels before acting on an issue.
- Keep issues concise and centered on the purpose, desired outcome, and problem
  or open question. Add acceptance criteria only when they clarify what done
  means.
- Avoid prescribing implementation details unless they are requirements or
  constraints. Decide the approach when implementation begins so it reflects
  the current code, tools, and constraints.
- Include source URLs or other evidence when needed to explain the purpose or
  constraints.
- Use GitHub's native issue dependencies for blocking relationships when they
  are available. Otherwise, record blockers in the issue body.
- Treat pull requests as implementation and review surfaces, not as substitutes
  for triage issues.

## Skill terminology

When a skill says to publish a ticket, create a GitHub Issue. When a skill says
to fetch a ticket, read the corresponding GitHub Issue. Prefer an available
GitHub integration and use the `gh` CLI as the fallback.
