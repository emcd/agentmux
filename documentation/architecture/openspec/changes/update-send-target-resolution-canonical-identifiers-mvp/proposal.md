# Change: Relock send target resolution to canonical identifiers only

## Why

Relay-side alias/display-name target resolution introduces ambiguity and can
conflict with future explicit address forms. We need deterministic send target
semantics anchored to canonical identifiers.

## What Changes

- Relock send explicit-target semantics to canonical identifiers only.
- Remove relay-side configured-name/display-name alias routing for send targets.
- Lock canonical send-path reject code for non-canonical/unknown explicit
  targets to `validation_unknown_target`.
- Remove alias-only ambiguity semantics (`validation_ambiguous_recipient`) from
  send-path contracts.
- Lock deterministic precedence when identifier namespaces overlap:
  - if a token matches both bundle member `session_id` and UI session id, the
    bundle member `session_id` wins.
- Preserve session `name` as informational display metadata only (not relay
  routable alias).

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
  - `tui-surface`
- Affected code (implementation follow-up):
  - relay explicit target resolution in send path
  - CLI/MCP/TUI target semantics/help text
  - send-path validation and integration tests

## Breaking Changes (pre-MVP intentional)

- Explicit targets that previously routed by configured session `name`/display
  alias will now fail with `validation_unknown_target`.
