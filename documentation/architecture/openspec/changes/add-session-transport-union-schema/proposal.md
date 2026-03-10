# Change: Add transport-aware session schema for tmux and ACP

## Why

Bundle session configuration is currently tmux-only. ACP feasibility work in
`todos/transport/2` confirmed a viable path for ACP-based session delivery, but
the existing `[[sessions]]` contract does not model transport alternatives.

We need a schema-level contract that supports both tmux and ACP while keeping a
safe migration path for current tmux bundles.

## What Changes

- Extend bundle session schema to support transport alternatives under
  `[[sessions]]` using a per-session transport descriptor.
- Canonicalize a tagged transport shape centered on `[sessions.transport]`.
- Define `tmux` and `acp` transport alternatives.
- Define minimal ACP descriptor fields for `stdio` and `http` variants.
- Define migration behavior for `format-version = 1` (legacy tmux) and
  `format-version = 2` (transport-aware).
- Keep this change strictly at the contract/spec level; no runtime or parser
  implementation is included in this pass.

## Non-Goals

- Implementing parser/runtime changes.
- Adding CLI flags or behavior changes.
- Changing MCP tool contracts in this proposal.
- Defining full ACP runtime adapter semantics beyond schema fields.

## Migration Notes

- Existing `format-version = 1` bundle files remain valid and continue to imply
  tmux transport.
- New transport-aware bundles use `format-version = 2`.
- In `format-version = 2`, omitted `[sessions.transport]` defaults to
  `kind = "tmux"` for low-friction migration.
- ACP transport is opt-in per session via explicit `kind = "acp"`.

## Impact

- Affected specs:
  - `session-relay`
- Expected implementation touchpoints (follow-up work):
  - `src/configuration.rs` bundle/session schema parsing and validation
  - relay transport abstraction for tmux/acp alternatives
  - unit and integration test coverage for v1/v2 schema handling

## Source References

- https://raw.githubusercontent.com/agentclientprotocol/agent-client-protocol/refs/heads/main/README.md
- https://agentclientprotocol.com/get-started/architecture.md
- https://agentclientprotocol.com/protocol/overview.md
- https://agentclientprotocol.com/protocol/prompt-turn.md
- https://agentclientprotocol.com/protocol/session-setup
- https://agentclientprotocol.com/protocol/transports
- https://agentclientprotocol.com/protocol/initialization
- https://github.com/agentclientprotocol/agent-client-protocol/blob/main/docs/protocol/draft/schema.mdx
