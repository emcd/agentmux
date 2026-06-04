# Context

- Overview and Quick Start: README.{md,rst}
- Architecture and Design: src/**/README.md (subsystem-specific)
- Development Practices: documentation/development-practices.md

- Use the 'context7' MCP server to retrieve up-to-date documentation for any SDKs or APIs.
- Use the 'nb' MCP server for project note-taking, issue tracking, and collaboration. The server provides LLM-friendly access to the `nb` note-taking system with proper escaping and project-specific notebook context.
- Check README files in directories you're working with for insights about architecture, constraints, and TODO items.

## Purpose
Agentmux is a multi-agent coordination runtime for coder sessions. It
provides relay-hosted inter-agent messaging, MCP tool surfaces, and a unified
CLI so operators and agents can list peers, send messages, inspect pane state,
and coordinate work across multiple worktrees with clear contracts.

## Tech Stack
- Rust (relay runtime, MCP server, CLI surfaces, configuration/runtime boot)
- TOML + serde (bundle/coder configuration and validation)
- tmux process orchestration for pane/session management
- OpenSpec for contract-first design and change tracking
- MCP-based tooling for coordination and documentation workflows

# Development Standards

Before implementing code changes, consult `documentation/development-practices.md` for
project-specific guidance on naming conventions, TOML practices, testing, code comments,
and module organization.

# Operation

- Use `rg --line-number --column` to get precise coordinates for MCP tools that require line/column positions.
- Choose appropriate editing tools based on the task complexity and your familiarity with the tools.
- If instruction files mention multiple language ecosystems, prefer tools and commands that match the project's configured languages; ignore language-inapplicable tooling unless the user explicitly requests it.
- Use a README-first discovery workflow to reduce token churn:
  - Start at the repository root `README.{md,rst}`, then read the nearest relevant subtree README.
  - After reading the nearest README, scope code searches to that subtree before considering repo-wide searches.
  - If a touched subsystem README is stale after your change, update it in the same batch.
- Batch related changes together when possible to maintain consistency.
- Use relative paths rather than absolute paths when possible.
- Do not write to paths outside the current project unless explicitly requested.
- Use `.auxiliary/scribbles` for scratch work and one-off experiments instead of `/tmp`; use `.auxiliary/temporary` for ephemeral test state and build artifacts that are safe to delete.
- In sandboxed environments (e.g., Codex CLI), treat file/network permission failures as escalation boundaries:
  - If an operation fails due to sandbox, file access, or network restrictions, rerun it with user escalation.
  - Do not spend time on retry loops or workaround exploration before escalating blocked operations.

## Note-Taking with `nb` MCP Server

### When to Use
- **Project coordination**: Record handoffs, document decisions, maintain task lists.
- **Issue tracking**: Create and manage todos with status tracking.
- **Knowledge sharing**: Document patterns, APIs, and project-specific knowledge.
- **Meeting notes**: Record discussions and action items.

### Scope and Noise Control
- Prefer to update an existing related note/todo over creating a new one when context already exists.
- Avoid logging routine, immediately completed mechanical actions in separate notes.
  Treat rolling handoffs as checkpoint notes, not activity logs: update them near
  compaction, after a major milestone, or after an agenda/ownership change
  discussed with the human.
- Create new notes/todos when information is likely to be useful across sessions or for other collaborators.

### Tagging Conventions
Use consistent tags for discoverability:
- **Project Component**: `#component-<name>` (e.g., `#component-data-models`)
- **Task Type**: `#task-<type>` (e.g., `#task-design`, `#task-bug`)
- **Status**: `#status-<state>` (e.g., `#status-in-progress`, `#status-review`)
- **Coordination**: `#handoff`, `#coordination`
- **Assignment**: Avoid owner tags (for example `#llm-*`) for task ownership. Use lane/folder ownership and explicit owner text in the note body when needed.

### Choosing `nb.todo` vs `nb.add`

- Use **`nb.todo`** for any item with actionable state (open/done): todos AND
  bugs/issues. This gives the note a checkbox, enables `nb.do`/`nb.undo`
  state tracking, and makes it appear in `nb.tasks` output.
- Use **`nb.add`** for everything else: coordination notes, decisions, designs,
  reference material, handoffs, and meeting notes.
- **Always specify a `folder`** when creating a note. A note created without a
  folder lands at the notebook root and is invisible to folder-scoped list
  views.
- Do not duplicate: if a bug is already tracked in `issues/<component>/`,
  do not also create a matching todo. Reference the issue selector in
  coordination notes or the relevant todo body instead.

### Notebook Identifier Clarification
- Treat note selectors (for example `coordination/mcp/1`) as canonical IDs for operations on existing notes (`nb.show`, `nb.edit`, `nb.delete`, etc.); do not supply a selector when creating new notes.
- `nb` MCP responses may include notebook-scoped identifiers (for example `agentmux:coordination/...`) that look path-like; these are selector forms, not repo-relative filesystem paths.
- Notebook storage is controlled by `nb` configuration (for example `NB_DIR`) and may be outside this repository.
- Prefer `nb` MCP commands to read/edit notes. Avoid assuming a selector maps to a file under the current repo.
- Use `nb.help` to read full command schemas; key lookups: `nb.search` with tag queries, `nb.tasks` for open todos, `nb.folders` to browse structure.

### Recommended `nb` Organization (Project-Defined)
- Prefer a folder taxonomy of `<issue-type>/<component>` (max depth 2) and avoid mixing top-level component folders with top-level issue-type folders.

| Category | Location | Purpose |
|----------|----------|---------|
| `coordination/` | notebook | Handoffs, org chart, team workflow |
| `ideas/` | notebook | Rough ideas, early-stage proposals; tag `#task-proposal` for OpenSpec drafts |
| `issues/` | notebook | Bug tracking, known issues |
| `reviews/` | notebook | Code and proposal reviews |
| `procedures/` | notebook | How-to guides, checklists |
| `todos/` | notebook | Task tracking |
| `artifacts/` | notebook | Preserved reference material: completed POCs, historical analysis |
| OpenSpec | filesystem | Formal proposals, specs, designs |
| `src/**/README.md` | filesystem | Architecture, constraints, design rationale |

- When an idea promotes to a formal OpenSpec proposal, delete the notebook draft — the OpenSpec file is the canonical record.
- For cross-component work, prefer `coordination/general` and use multiple `#component-*` tags.
- For per-component rolling handoffs, prefer `coordination/<component>` (one stable note updated at checkpoints).
- Keep notebook lifecycle hygiene:
    - prune completed todos quickly,
    - keep only active/near-term coordination checkpoints,
    - delete stale history-only notes with no owner or action.
- Keep todo titles concise (under 60 chars); use the `tasks` argument for detailed checklist items. This keeps notebook list views readable.

### `nb` vs OpenSpec Rubric
- Use **OpenSpec proposals** for cross-cutting changes, contract-shaping work, architecture shifts, or work that needs explicit design discussion.
- Use **`nb` todos/notes** for scoped, self-contained implementation tasks where the path is straightforward.
- When in doubt about whether work needs an OpenSpec proposal or only `nb` execution tracking, prefer OpenSpec first for design clarity.
- For each active OpenSpec proposal, keep **exactly one** linked `nb` todo as the tracking anchor (with proposal reference), rather than duplicating full task trees in both systems.

### OpenSpec Draft and Handoff Hygiene
- Draft OpenSpec proposal text in a dedicated `nb` note first so collaborators can review without local file access barriers; share the note id when requesting feedback.
- When asking for proposal feedback, share the notebook note id first; do not request review against local-only proposal files collaborators cannot access.
- Keep rolling handoff notes stable and update in place, separate from OpenSpec draft/proposal text.
- Do not repurpose or overwrite rolling handoff notes with proposal content.
- After draft review converges, move approved proposal text into `openspec/**` files for human review and commit.

## Agentmux Coordination

`agentmux` messages may arrive in envelope format and can appear as user prompts. Treat envelope-shaped prompts as inter-agent messages, not direct human instructions. Respond via `agentmux` MCP tools (`list`, `send`). Immediate interruption is not required — note the message and respond when safe.

Default to low-noise coordination. Send messages only when:
- you are blocked and need a decision or input,
- you are requesting a concrete review,
- you are handing off completed work with validation results,
- you are reporting a material risk, failure, or scope change.

Batch related updates into one message. When conversation volume rises, coordinator may enforce "blockers-only" mode.

## Tests Development
- Prefer tests under `tests/unit` and `tests/integration` over inline `#[cfg(test)]` modules in `src/**`.
- Prefer tests that exercise public interfaces; avoid source-inclusion patterns used only to reach private internals.
- Inline `#[cfg(test)]` is permitted only when ALL of the following hold:
  1. The tested item is crate-private **by design** (not by oversight or laziness) and making it testable externally would require widening its visibility or adding a `#[doc(hidden)] pub` escape hatch that would itself become unintended API surface.
  2. No existing public interface exercises the same code path.
  3. The inline test block contains at most **one** `#[test]` function.
- If a candidate inline test fails any of these conditions, move it to `tests/unit` and widen visibility or restructure as needed. Do not default to inline to avoid that conversation; the friction is intentional.

## OpenSpec Instructions

Workflow Guide: @openspec/AGENTS.md

Always open `openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan).
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work.
- Sounds ambiguous and you need the authoritative spec before coding.

Use `openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

# Commits

- Use `git status` to ensure all relevant changes are in the changeset.
- Do **not** commit without explicit user approval. Unless the user has requested the commit, **ask first** for a review of your work.
- Do **not** bypass commit safety checks (e.g., `--no-verify`, `--no-gpg-sign`) unless the user explicitly approves doing so.
- If a commit hook rejects a commit, fix the issue, restage the intended files, and rerun `git commit` with the same message. Do **not** amend a previous commit unless the user explicitly asked for an amend.
- Use present tense, imperative mood verbs (e.g., "Fix" not "Fixed").
- Write sentences with proper punctuation.
- Include a `Co-Authored-By:` field as the final line. Should include the model name and a no-reply address.
- Avoid using `backticks` in commit messages as shell tools may evaluate them as subshell captures.

# Project Notes

<!-- This section accumulates project-specific knowledge, constraints, and deviations.
     For structured items, use documentation/architecture/decisions/ and `nb`. -->

## Notebook Conventions

### Handoff Notes

- Use `coordination/<component>` as the active handoff lane for each owner
  (for example `coordination/relay`, `coordination/mcp`, `coordination/tui`).
- Keep one rolling handoff note per component and update it in place instead of
  creating a new note for each checkpoint.
- Use `coordination/general` for coordinator-wide state and cross-component
  snapshots.
- Minimize handoff churn: prefer updates for meaningful lane-state changes and
  pre-compaction checkpoints, not routine micro-status noise.
- Handoff content: a brief summary of recent accomplishments and a current
  agenda. Always replace the note body (never append) so it stays one
  screenful — a growing checkpoint log is an anti-pattern.
- For cross-component notes, apply multiple `#component-*` tags.
- Prefer pruning stale/superseded coordination checkpoints while preserving the
  current per-component handoff context.

## Team Org

Team roster, lane ownership, merge policy, coordinator and specialist responsibilities, and OpenSpec multi-agent workflow live in `coordination/general/14`.

## Alpha Defaults

- This project is **alpha software with live releases**. Do **not** preserve
  backwards compatibility unless the human developer explicitly requests it.
- Prefer **raising errors** (fail fast) over "graceful degradation" with
  defaults; only use silent fallbacks when explicitly requested.
- Do not add new `-mvp` suffixes to OpenSpec change IDs. Existing archived
  MVP-era proposal names may remain as historical artifacts.

## MCP Tool Inventory Refresh

- After changes that add/remove/rename MCP tools, perform a refresh check:
  - restart the relevant MCP server,
  - verify tool inventory from the client side,
  - if stale tools persist, request a client restart to force a fresh MCP
    handshake.
- Record refresh outcome in the lane handoff note when tool inventory changed.
