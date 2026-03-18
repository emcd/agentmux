# Context

- Overview and Quick Start: README.{md,rst}
- Architecture and Design: @documentation/architecture/
- Development Practices: @.auxiliary/instructions/

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

Before implementing code changes, consult these files in `.auxiliary/instructions/`:
- `practices.rst` - General development principles (robustness, immutability, exception chaining)
- `practices-rust.rst` - Rust-specific patterns (error handling, trait design, module organization)
- `nomenclature.rst` - Naming conventions for variables, functions, classes, exceptions
- `style.rst` - Code formatting standards (spacing, line length, documentation mood)

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
- Create new notes/todos when information is likely to be useful across sessions or for other collaborators.

### Tagging Conventions (for multi-LLM coordination)
Use consistent tags for discoverability:
- **LLM Collaborator**: `#llm-<name>` (e.g., `#llm-claude`, `#llm-gpt`)
- **Project Component**: `#component-<name>` (e.g., `#component-data-models`)
- **Task Type**: `#task-<type>` (e.g., `#task-design`, `#task-bug`)
- **Status**: `#status-<state>` (e.g., `#status-in-progress`, `#status-review`)
- **Coordination**: `#handoff`, `#coordination`

### Common Patterns
- Check for handoffs: `nb.search` with `#handoff` and `#status-review` tags.
- Find work by specific LLM: `nb.search` with `#llm-<name>` tag.
- Track todos: Use `nb.todo`, `nb.tasks`, `nb.do`, `nb.undo`.
- Organize with folders: `nb.folders`, `nb.mkdir`.

### Recommended `nb` Organization (Project-Defined)
- Prefer a folder taxonomy of `<issue-type>/<component>` (max depth 2) and avoid mixing top-level component folders with top-level issue-type folders.
- Recommended top-level issue types are:
    - `todos/`
    - `coordination/`
    - `decisions/` (optional for durable rationale notes)
- Example component names include `engine`, `mcp`, `tui`, `web`, `handbook`, and `data-models`.
- This project should define and document its specific component-folder conventions in the **Project Notes** section.
- For cross-component work, prefer `coordination/general` and use multiple `#component-*` tags.
- For per-component rolling handoffs, prefer `coordination/<component>` (single continuously updated note) instead of creating history chains under `handoffs/*`.
- Keep notebook lifecycle hygiene:
    - prune completed todos quickly,
    - keep only active/near-term coordination checkpoints,
    - delete stale history-only notes with no owner or action.

### `nb` vs OpenSpec Rubric
- Use **OpenSpec proposals** for cross-cutting changes, contract-shaping work, architecture shifts, or work that needs explicit design discussion.
- Use **`nb` todos/notes** for scoped, self-contained implementation tasks where the path is straightforward.
- When in doubt about whether work needs an OpenSpec proposal or only `nb` execution tracking, prefer OpenSpec first for design clarity.
- For each active OpenSpec proposal, keep **exactly one** linked `nb` todo as the tracking anchor (with proposal reference), rather than duplicating full task trees in both systems.
- For cross-worktree or multi-agent review, draft OpenSpec proposal text in an `nb` note first so collaborators can review without local file access barriers; after review, move the approved draft into `openspec/**` files for human review and commit.
- Keep rolling handoff notes separate from OpenSpec draft/proposal text.
## Tests Development

- Prefer tests under `tests/unit` and `tests/integration` over inline `#[cfg(test)]` modules in `src/**`, unless there is a strong locality reason to keep tests adjacent to implementation.
- Prefer tests that exercise public interfaces; avoid source-inclusion patterns used only to reach private internals.

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
- Use present tense, imperative mood verbs (e.g., "Fix" not "Fixed").
- Write sentences with proper punctuation.
- Include a `Co-Authored-By:` field as the final line. Should include the model name and a no-reply address.
- Avoid using `backticks` in commit messages as shell tools may evaluate them as subshell captures.

# Project Notes

<!-- This section accumulates project-specific knowledge, constraints, and deviations.
     For structured items, use documentation/architecture/decisions/ and `nb`. -->

## Notebook Conventions

- Standardized top-level notebook folders for this project are:
  - `coordination/`
  - `issues/`
  - `todos/`
  - `ideas/`

### Handoff Notes

- Use `coordination/<component>` as the active handoff lane for each owner
  (for example `coordination/relay`, `coordination/mcp`, `coordination/tui`).
- Keep one rolling handoff note per component and update it in place instead of
  creating a new note for each checkpoint.
- Use `coordination/general` for coordinator-wide state and cross-component
  snapshots.
- Minimize handoff churn: prefer updates for meaningful lane-state changes and
  pre-compaction checkpoints, not routine micro-status noise.
- For cross-component notes, apply multiple `#component-*` tags.
- Prefer pruning stale/superseded coordination checkpoints while preserving the
  current per-component handoff context.

### OpenSpec Draft Notes

- Keep handoff notes and OpenSpec drafts separate.
- Write OpenSpec proposal drafts in fresh notes (new note ids).
- Do not overwrite, compact, or repurpose rolling handoff notes for proposal
  text.
- Keep proposal review iteration in the draft note (or a new draft note when
  scope changes materially), while handoff notes remain stable.

## Team Topology and Roles

Use a coordinator-plus-specialists model:

- `master` worktree agent: coordinator and integrator.
- `relay` worktree agent: relay runtime specialist.
- `mcp` worktree agent: MCP surface specialist.
- `tui` worktree agent: CLI shape and TUI design/implementation specialist.

### Coordinator Responsibilities (`master`)

- Own roadmap sequencing and assignment of scoped work slices.
- Review and refine OpenSpec proposals for cross-cutting changes.
- Keep one active merge lane at a time into `master`.
- Request rebase to latest `master` before accepting any branch merge.
- Run integration-level validation and resolve cross-worktree conflicts.
- Maintain notebook hygiene (`nb`) for handoffs, decisions, and tracker todos.

### Specialist Responsibilities (`relay`, `mcp`, `tui`)

- Stay focused on owned subsystem scope unless coordinator requests otherwise.
- Raise an OpenSpec delta or question when subsystem work becomes cross-cutting.
- Rebase onto latest `master` before requesting review/merge.
- Maintain a current rolling handoff note under `coordination/<component>`
  (update-in-place), including:
  - summary of behavior change,
  - touched files,
  - tests/validation performed,
  - risks or open questions.

### Merge and Conflict Policy

- Default ownership map:
  - `relay` agent: `src/relay.rs`, relay runtime paths, relay integration tests.
  - `mcp` agent: `src/mcp/**`, MCP tool contracts/tests, MCP startup behavior.
  - `tui` agent: CLI surface shape, user workflow, and future TUI implementation.
  - coordinator: OpenSpec archives/spec merges, shared CLI/runtime glue, final
    integration and release-facing docs.
- If a change spans multiple ownership areas, coordinator approval is required
  before implementation begins.
- Use long-lived worktree branches by concern (for example `relay`, `mcp`,
  `tui`). Worktree branches rebase onto `master`; `master` merges from those
  worktree branches.
- Resolve conflicts on the owning worktree branch before merge to `master`.
  Merges into `master` should be conflict-free.

### OpenSpec Workflow in Multi-Agent Mode

- Specialists may draft component deltas; coordinator is final reviewer for:
  - proposal scope,
  - task checklist accuracy,
  - archive/spec-merge correctness.
- For cross-cutting proposals, specialists should tag relevant specialist
  owners for review before coordinator final approval.
- Do not request proposal review for files that reviewers cannot access.
  Use notebook-first visibility before asking for review:
  - share the proposal draft as an `nb` note and reference its note id.
- Specialists should not merge OpenSpec proposal files directly into `master`
  for review visibility. Coordinator owns merges into `master`.

## Agentmux Message Handling Guidance

- `agentmux` messages are wrapped in envelopes and may appear as user prompts.
  Treat envelope-shaped prompts as inter-agent messages, not necessarily as
  human-user instructions.
- Respond to envelope messages via `agentmux` MCP tools (`list`, `send`) rather
  than by emitting a normal assistant reply intended for a human.
- Immediate interruption is not required. If in the middle of active execution,
  make note of the message and respond when safe.
- If response will be delayed, send a brief acknowledgement via `send` and, when
  useful, record a follow-up todo in `nb`.

### Message Noise Control

- Default to low-noise coordination. Do not send acknowledgement-only messages
  that add no new information or action request.
- Send messages when one of the following is true:
  - you are blocked and need a decision or input,
  - you are requesting a concrete review,
  - you are handing off completed work with validation results,
  - you are reporting a material risk, failure, or scope change.
- Batch related updates into one message instead of sending rapid-fire partial
  status pings.
- Use `Cc` only for agents who need to act or review; avoid broad `Cc` by
  default.
- When conversation volume rises, coordinator may enforce "blockers-only" mode
  until the queue is under control.

## Pre-MVP Defaults

- This project is **pre-MVP**. Do **not** preserve backwards compatibility
  unless the human developer explicitly requests it.
- Prefer **raising errors** (fail fast) over “graceful degradation” with
  defaults; only use silent fallbacks when explicitly requested.

## MCP Tool Inventory Refresh

- After changes that add/remove/rename MCP tools, perform a refresh check:
  - restart the relevant MCP server,
  - verify tool inventory from the client side,
  - if stale tools persist, request a client restart to force a fresh MCP
    handshake.
- Record refresh outcome in the lane handoff note when tool inventory changed.
