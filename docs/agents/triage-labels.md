# Triage labels

The engineering skills use five canonical triage roles. The same strings are
used as GitHub labels by default.

| Canonical role    | GitHub label      | Meaning                                           |
| ----------------- | ----------------- | ------------------------------------------------- |
| `needs-triage`    | `needs-triage`    | Maintainer review and classification are required |
| `needs-info`      | `needs-info`      | More information is required from the reporter    |
| `ready-for-agent` | `ready-for-agent` | An agent can implement the fully specified issue  |
| `ready-for-human` | `ready-for-human` | Human judgment or implementation is required      |
| `wontfix`         | `wontfix`         | The issue will not be actioned                    |

When a skill refers to a canonical triage role, apply the corresponding GitHub
label from this table. Repositories that use different labels should update the
mapping while preserving the canonical roles.
