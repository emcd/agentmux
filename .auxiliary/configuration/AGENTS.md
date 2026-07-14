# Context

- Overview and Quick Start: README.{md,rst}
- Architecture and Design: src/**/README.md (subsystem-specific)
- Development Practices: documentation/development-practices.md

- Use the 'context7' MCP server to retrieve up-to-date documentation for any SDKs or APIs.
- Use the 'nb' MCP server for project note-taking, issue tracking, and collaboration. The server provides LLM-friendly access to the `nb` note-taking system with proper escaping and project-specific notebook context.
- Check README files in directories you're working with for insights about architecture and design decisions.

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

## Prerequisites

- Rust (stable; minimum version 1.90 per Cargo.toml's `rust-version` --
  no exact toolchain pin; CI and local builds float on whatever
  `stable` resolves to at build time).
- tmux (only required when working on the Tmux transport — the
  canonical `cargo nextest run` invocation exercises the Tmux commands
  directly via integration tests).
- **Zig 0.15.x** (only required when building with the `pty` Cargo
  feature, since `libghostty-vt`'s build script requires it; CI
  installs it via `setup-zig`). Default `cargo build` and the canonical
  `cargo nextest run` do NOT invoke Zig and do NOT require it on `PATH`.
- For libghostty-vt's vendored ghostty clone (only when building with
  `--features pty` without a local override): outbound network access
  to `github.com/ghostty-org/ghostty.git`. To bypass the network clone,
  set `GHOSTTY_SOURCE_DIR` to a pre-checked-out ghostty source tree
  that contains `build.zig`; to bypass Zig's package fetch, set
  `GHOSTTY_ZIG_SYSTEM_DIR` to a directory containing the Zig package
  cache.
- The `libghostty-vt-sys/pkg-config` feature (point at an installed
  `libghostty-vt.a` via pkg-config, skipping the vendored Zig build
  entirely) is currently unreachable through agentmux's consumer
  dep `libghostty-vt = "=0.2.0"` because the safe wrapper does not
  re-export that feature. See `documentation/development/README.md`
  Zig-free Pty Builds for the recommended escape hatches today and
  the upstream-PR gap for unlocking `pkg-config`.

# Development Standards

Before implementing code changes, consult `documentation/development-practices.md` for
project-specific guidance on naming conventions, TOML practices, testing, code comments,
and module organization.

# Operation

- Use a README-first discovery workflow to reduce token churn:
  - Start at the repository root `README.{md,rst}`, then read the nearest relevant subtree README.
  - After reading the nearest README, scope code searches to that subtree before considering repo-wide searches.
  - If a touched subsystem README is stale after your change, update it in the same batch.
- Use relative paths rather than absolute paths when possible (relative paths are less likely to trigger tool call permission requests).
- Do not write to paths outside the current project unless explicitly requested.
- Use `.auxiliary/scribbles` for scratch work and one-off experiments instead of `/tmp`.
- Use `.auxiliary/temporary` for ephemeral test state and build artifacts that are safe to delete.
- In sandboxed environments (e.g., Codex CLI), treat file/network permission failures as escalation boundaries:
  - If an operation fails due to sandbox, file access, or network restrictions, rerun it with user escalation.
  - Do not spend time on retry loops or workaround exploration before escalating blocked operations.
- When writing here-docs or multi-line shell strings, suppress expansions by quoting the delimiter (e.g., `'EOF'` instead of `EOF`) unless you intentionally need variable or command substitution.

## Guidance Files

| Topic | File |
|-------|------|
| `nb` MCP tools, tagging, and notebook organization | @documentation/agents/notebook.md |
| OpenSpec proposals and workflow | @documentation/agents/openspec.md |
| Delegated review flow and stacked commits | @documentation/agents/reviews.md |

### Recommended Organization

| Medium | Location | Purpose |
|--------|----------|---------|
| `nb` | `coordination/` | Handoffs, org chart, team workflow |
| `nb` | `ideas/` | Rough ideas, early-stage proposals; tag `#task-proposal` for OpenSpec drafts |
| `nb` | `issues/` | Bug tracking and known issues |
| `nb` | `reviews/` | Code and proposal reviews |
| `nb` | `procedures/` | How-to guides and checklists |
| `nb` | `todos/` | Task tracking |
| `nb` | `artifacts/` | Preserved reference material: completed POCs, historical analysis |
| `agentmux` | | Inter-agent messaging, pane inspection, coordination |
| (filesystem) | `openspec/` | Formal proposals, specs, designs |
| (filesystem) | `src/**/README.md` | Architecture, constraints, design rationale |

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

This project uses OpenSpec 1.x (OPSX), the action-based workflow. OPSX skills
deliver workflow instructions through the agentsmgr distribution pipeline.

Workflow skills: `opsx-propose`, `opsx-explore`, `opsx-apply`,
`opsx-sync`, `opsx-archive`.

Use OPSX skills when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan).
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work.
- Sounds ambiguous and you need the authoritative spec before coding.

CLI state queries: `openspec list`, `openspec list --specs`,
`openspec status --change <id>`, `openspec validate --all --strict`.

When a commit completes an OpenSpec task or requirement, update the relevant OpenSpec task status in the same commit.

# Commits

- Use `git status` to ensure all relevant changes are in the changeset.
- Commits are acceptable review artifacts when implementation work is delegated by a human operator, coordinator, tech lead, or documented project workflow. Otherwise, ask before committing.
- Do **not** merge, push, publish review branches, or modify shared branches without explicit human approval.
- Do **not** bypass commit safety checks (e.g., `--no-verify`, `--no-gpg-sign`) unless the user explicitly approves doing so.
- If a commit hook rejects a commit, assume no commit was created unless Git clearly reports otherwise. Fix the issue, restage the intended files, and rerun `git commit` with the same message. Do **not** amend a *different, already-existing* commit as a workaround for a rejected attempt — that risks destroying the boundary of unrelated prior work.
- Use present tense, imperative mood verbs (e.g., "Fix" not "Fixed").
- Write sentences with proper punctuation.
- Include a `Co-Authored-By:` field as the final line. Should include the model name and a no-reply address.
- Avoid using `backticks` in commit messages as shell tools may evaluate them as subshell captures. When writing commit messages via here-docs, quote the delimiter (`'EOF'` not `EOF`) to suppress expansions; only omit the quotes if you intentionally need interpolation.

## Delegated Review and Stacked Commits

**Read this section before reviewing or stacking commits.** @documentation/agents/reviews.md covers the delegated review flow, review request packet format, and how to handle stacked commits with `--fixup`/`--autosquash`.

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

## Procedures

- [Cleanup and removal protocol](agentmux:procedures/2) — inventory-first, spec-coupled deltas, proof-of-absence gate for all removal/cleanup directives.

## Alpha Defaults

- This project is **alpha software with live releases**. Do **not** preserve
  backwards compatibility unless the human developer explicitly requests it.
- Prefer **raising errors** (fail fast) over "graceful degradation" with
  defaults; only use silent fallbacks when explicitly requested.
- When a field, flag, parameter, or variant is dropped, delete it outright.
  Do not add explicit rejection logic, a dedicated error code, or unit/
  integration tests asserting the removed thing is now rejected or absent.
  Existing unknown-field/unknown-flag validation (`deny_unknown_fields`,
  clap's unknown-flag error, etc.) already covers it without bespoke
  machinery. This applies to OpenSpec requirements too: do not add a
  "Reject X" scenario for a removed surface.
- Do not add new `-mvp` suffixes to OpenSpec change IDs. Existing archived
  MVP-era proposal names may remain as historical artifacts.

## MCP Tool Inventory Refresh

- After changes that add/remove/rename MCP tools, perform a refresh check:
  - restart the relevant MCP server,
  - verify tool inventory from the client side,
  - if stale tools persist, request a client restart to force a fresh MCP
    handshake.
- Record refresh outcome in the lane handoff note when tool inventory changed.
