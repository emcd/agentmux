# OpenSpec Instructions

This project uses OpenSpec 1.x (OPSX), the action-based workflow. OPSX skills
deliver workflow instructions through the agentsmgr distribution pipeline.

## Workflow

- `opsx-propose` -- create a complete change proposal in one step.
- `opsx-explore` -- think through ideas before committing to a change.
- `opsx-apply` -- implement tasks from an approved change.
- `opsx-sync` -- sync delta specs into main specs.
- `opsx-archive` -- archive a completed change.

## CLI State Queries

- `openspec list` and `openspec list --specs` -- active changes and specs.
- `openspec status --change <id>` -- artifact state for a change.
- `openspec instructions <artifact> --change <id>` -- authoring instructions.
- `openspec validate --all --strict` -- validate changes and specs.

## Conventions

- Project configuration: `openspec/config.yaml` (default schema:
  `spec-driven`). Custom workflow schemas may be added under
  `openspec/schemas/` as the project evolves.
- Change IDs: kebab-case, verb-led (`add-`, `update-`, `remove-`,
  `refactor-`).
- Every requirement uses SHALL/MUST and carries at least one
  `#### Scenario:` (exactly four hashtags).
- When a commit completes an OpenSpec task or requirement, update the
  relevant task status in the same commit.
- When a commit narrows or otherwise changes a requirement's wording,
  sweep `design.md` and `proposal.md` for the same claim, not just the
  delta spec and `tasks.md`. `openspec validate --strict` checks
  structure, not agreement between a proposal's own documents, so a
  requirement can be narrowed in the delta while its own design
  rationale still asserts the wider version — a self-contradiction
  invisible to both the validator and a reviewer scoped to the delta
  alone.

Treat OpenSpec proposals like code: commit proposal files to a branch,
reviewers review the commit (`git show`), author amends as needed, merge when
settled. No notebook draft step.
