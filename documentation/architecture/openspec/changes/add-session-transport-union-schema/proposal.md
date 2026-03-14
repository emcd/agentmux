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
- Keep `coder-session-id` as session-level identity state used for tmux resume
  and ACP session load semantics.
- Preserve existing bundle membership invariants:
  - unique session IDs per bundle,
  - unique optional session names per bundle,
  - rejection of unknown coder references.
- Define ACP coder descriptor fields with:
  - `channel` (`stdio` | `http`)
  - for `channel = "stdio"`:
    - required `command` (string command template)
  - for `channel = "http"`:
    - required `url`
    - optional `headers` entries (`name`, `value`)
- Define ACP lifecycle selection from session state:
  - session with `coder-session-id` -> ACP `session/load`
  - session without `coder-session-id` -> ACP `session/new`
  - ACP load failure is fail-fast and MUST NOT silently fall back to ACP
    `session/new` in the same operation.
- Move tmux-specific startup/readiness fields into `[coders.tmux]`.
- Exclude `tui` from this proposal; TUI is treated as a separate category.

## Non-Goals

- Changing CLI/MCP command surfaces.
- Defining TUI session category/schema in this proposal.
- Defining ACP `look` synthesized snapshot behavior in this proposal
  (tracked as separate follow-up).

## Concise Migration Notes

1. Move configuration files to `format-version = 2` for this contract.
2. For each coder, define exactly one target table:
   `[coders.tmux]` or `[coders.acp]`.
3. For tmux coders, place command/readiness keys under `[coders.tmux]`.
4. For ACP coders, place transport descriptors under `[coders.acp]`:
   - stdio uses single string `command`
   - http uses `url` and optional `headers`.
5. Remove ACP `session-mode` usage from configuration.
6. Sessions continue to reference coders; `coder-session-id` selects ACP load
   path and tmux resume-command behavior.
7. Existing bundle membership invariants remain enforced during migration
   (unique IDs/names and known coder references).

## Impact

- Affected specs:
  - `session-relay`
- Expected follow-up implementation touchpoints:
  - `src/configuration.rs` (ACP descriptor validation updates)
  - runtime ACP lifecycle selector and load-failure handling in relay paths
  - relay/runtime target abstraction for coder-backed tmux/acp execution

## Source References

- https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/refs/heads/main/README.md
- https://agentclientprotocol.com/get-started/agents.md
- https://agentclientprotocol.com/get-started/architecture.md
- https://agentclientprotocol.com/protocol/overview.md
- https://agentclientprotocol.com/protocol/prompt-turn.md
- https://agentclientprotocol.com/protocol/session-setup.md
- https://agentclientprotocol.com/protocol/transports
- https://agentclientprotocol.com/protocol/initialization
- https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/draft/schema.mdx
