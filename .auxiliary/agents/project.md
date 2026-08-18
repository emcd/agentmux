# Project Guidance

Project-owned knowledge the generated `AGENTS.md` entrypoint must not own.
Structured tracking stays in `nb`.

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

## Additional Context

- Architecture and Design: `src/**/README.md` (subsystem-specific)
- Development Practices: `documentation/development-practices.md` — consult
  before implementing code changes, for project-specific guidance on naming
  conventions, TOML practices, testing, code comments, and module
  organization. This supplements, and is distinct from, the generic
  language standards in `.auxiliary/agents/standards/`.

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

## Team Org

Team roster, lane ownership, merge policy, review routing, coordinator and
specialist responsibilities, and OpenSpec multi-agent workflow live in
`coordination/general/14`.

## Procedures

- [Cleanup and removal protocol](agentmux:procedures/2) — inventory-first,
  spec-coupled deltas, proof-of-absence gate for all removal/cleanup
  directives.

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
- Record refresh outcome in the lane handoff note when tool inventory
  changed.

## Notebook Notes

- **Milestone**: `#milestone-<version>` (e.g., `#milestone-0-9-0`, hyphens not
  dots) marks an item's release allocation. This is the sole source of truth
  for milestone membership — milestone notes (`coordination/general/16`,
  `/18`, ...) hold narrative only and must not restate item-by-item lists.
  Query membership with `nb_list(tags: ["milestone-0-9-0"])`; see
  `procedures/general/1` in the notebook for the full convention and
  rationale. Exception: OpenSpec proposal ids (`add-*`) can't carry nb tags,
  so their milestone allocation stays in the relevant milestone note's prose.
  "Narrative" is scoped narrowly: theme, arc-level operator directives, and
  OpenSpec proposal allocations only. Dispatch status, investigation logs,
  triage tables, and checkpoint-by-checkpoint history do not belong here even
  in prose form — that content belongs on the individual item's own note (its
  detailed history/evidence), or in the relevant rolling handoff if it's
  short-lived operating state. A milestone note that grows into an
  append-only chronicle of everything that happened is the same anti-pattern
  the Handoff Hygiene section (`.auxiliary/agents/procedures/notebook.md`)
  warns against for handoff notes, just relocated — trim it back to
  narrative rather than letting it accumulate.

## OpenSpec Notes

- When a commit narrows or otherwise changes a requirement's wording, sweep
  `design.md` and `proposal.md` for the same claim, not just the delta spec
  and `tasks.md`. `openspec validate --strict` checks structure, not
  agreement between a proposal's own documents, so a requirement can be
  narrowed in the delta while its own design rationale still asserts the
  wider version — a self-contradiction invisible to both the validator and a
  reviewer scoped to the delta alone.

## Notes

<!-- Accumulate project-specific knowledge, constraints, deviations, and durable
     links here. For structured items, use `nb`. -->
