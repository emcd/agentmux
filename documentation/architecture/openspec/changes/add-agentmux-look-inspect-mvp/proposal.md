# Change: Add agentmux look MVP

## Why

Operators currently inspect other sessions via ad-hoc tmux commands during
debugging and coordination. That workflow is inconsistent, difficult to
automate, and lacks explicit policy and audit semantics.

## What Changes

- Add a new CLI command: `agentmux look <target-session>`.
- Add a new MCP tool: `look`.
  - This is an explicit, stable exception to current MCP delivery verbs
    (`list`, `send`) because inspection is a read-only snapshot operation, not
    a delivery operation.
  - Contract wording is expected to align with the MCP naming baseline from
    `mcp/11` and its OpenSpec sync follow-up tracker (`todos/mcp/14`).
- Add a relay-level read operation (`look`) used by both CLI and MCP.
- Keep inspection scoped to local same-bundle usage for MVP.
- Define deterministic line-window behavior (`default=120`, `max=1000`) and
  stable validation errors.
- Defer authorization policy and permission-deny behavior to a dedicated
  follow-up authorization spec.

## Impact

- Affected specs:
  - `cli-surface`
  - `mcp-tool-surface`
  - `session-relay`
- Affected code:
  - CLI command parsing and output rendering
  - MCP tool surface and schema
  - Relay request handling
