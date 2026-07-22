# Issue tracker: GitHub Issues

Issues for this repository are tracked in GitHub Issues. Infer the repository
from the configured Git remote instead of hard-coding an owner or repository
name in agent instructions.

## Conventions

- Create implementation issues and review-follow-up issues as GitHub Issues.
- Read the full issue body, comments, and labels before acting on an issue.
- Include enough context for an agent to act: source URL, affected files or
  lines when available, problem statement, and expected resolution or open
  question.
- Use GitHub's native issue dependencies for blocking relationships when they
  are available. Otherwise, record blockers in the issue body.
- Treat pull requests as implementation and review surfaces, not as substitutes
  for triage issues.

## Skill terminology

When a skill says to publish a ticket, create a GitHub Issue. When a skill says
to fetch a ticket, read the corresponding GitHub Issue. Prefer an available
GitHub integration and use the `gh` CLI as the fallback.
