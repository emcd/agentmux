# Change: Add coder-target schema for tmux and ACP sessions

## Why

Session entries already reference coders. The target model should therefore be a
property of coder definitions, not duplicated per session.

This enables reuse, reduces repeated configuration, and keeps session files
focused on routing identity and coder association.

## What Changes

- Adopt direct class-specific target tables on `[[coders]]` (not sessions):
  - `[coders.tmux]`
  - `[coders.acp]`
- Require exactly one target table per coder.
- Keep sessions referencing coders via `sessions.coder`.
- Keep `coder-session-id` as session-level data for per-session identity state.
- Define ACP coder descriptor fields with:
  - `channel` (`stdio` | `http`)
  - optional `session-mode` (`new` | `load`, default `new`)
  - `session-mode = "load"` requires each referencing session to provide
    `coder-session-id`.
- Move tmux-specific startup/readiness fields into `[coders.tmux]`.
- Exclude `tui` from this proposal; TUI is treated as a separate category.
- Keep this as contract/spec only (no implementation in this pass).

## Non-Goals

- Implementing parser/runtime changes.
- Changing CLI/MCP command surfaces.
- Defining TUI session category/schema in this proposal.

## Concise Migration Notes

1. Move configuration files to `format-version = 2` for this contract.
2. For each coder, define exactly one target table:
   `[coders.tmux]` or `[coders.acp]`.
3. For tmux coders, place command/readiness keys under `[coders.tmux]`.
4. For ACP coders, place connection/lifecycle defaults under `[coders.acp]`.
5. Sessions continue to reference coders; `coder-session-id` remains the
   per-session identity token for resume/load semantics.

## Impact

- Affected specs:
  - `session-relay`
- Expected follow-up implementation touchpoints:
  - `src/configuration.rs` (`RawCoder` one-of target validation + imputation)
  - session validation against referenced coder target class
  - relay/runtime target abstraction for coder-backed tmux/acp execution

## Source References

- https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/refs/heads/main/README.md
- https://agentclientprotocol.com/get-started/architecture.md
- https://agentclientprotocol.com/protocol/overview.md
- https://agentclientprotocol.com/protocol/prompt-turn.md
- https://agentclientprotocol.com/protocol/session-setup
- https://agentclientprotocol.com/protocol/transports
- https://agentclientprotocol.com/protocol/initialization
- https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/draft/schema.mdx
