## Context

Operators need a supported way to inspect another session snapshot for
debugging and coordination without manual tmux commands.

The MVP should maximize operational value while avoiding cross-bundle
complexity.

## Goals

- Add one public verb for inspection across operator surfaces (`look`).
- Provide one relay-level canonical inspection payload for CLI and MCP.
- Keep MVP trust boundary and scope conservative:
  - same host
  - same user
  - same bundle
- Define deterministic bounds for line-window capture and errors.

## Non-Goals

- Cross-bundle inspection in MVP.
- Streaming/watch inspection mode in MVP.
- Historical search or log browsing in MVP.
- Primary `snoop` user-facing command in MVP.
- Authorization policy and permission model changes in MVP.

## Decisions

- Decision: use `look` as the public surface verb.
  - CLI command: `agentmux look <target-session>`
  - MCP tool name: `look`
  - Relay operation name: `look`.
- Decision: keep MCP `look` as an explicit naming exception to MCP delivery
  verbs.
  - Rationale: `send` remains the delivery verb; inspection is a read-only
    snapshot category and should not be encoded as a send-family extension.
  - Stability rule: MCP surface uses `list` and `send` for delivery workflows,
    and `look` for inspection workflows.
- Decision: keep relay look scope same-bundle only in MVP.
  - If a caller attempts cross-bundle scope, return
    `validation_cross_bundle_unsupported`.
- Decision: inspection payload uses `snapshot_lines: string[]`.
- Decision: `bundle_name` in relay look request is optional/redundant when
  bundle context is already bound by association/socket path.
- Decision: deterministic line capture bounds:
  - default: `120`
  - max: `1000`
  - out-of-range => `validation_invalid_lines`.
- Decision: keep authorization out of this change.
  - This OpenSpec change defines inspection command/contract semantics only.
  - Authorization semantics are deferred to a dedicated follow-up spec.

## Risks / Trade-offs

- Deferring authorization means this change does not define deny-policy
  behavior.
  - Mitigation: keep scope explicit and track authorization as a separate spec.

## Migration Plan

1. Add relay `look` request handling and response contract.
2. Add MCP `look` tool mapped to relay `look`.
3. Add CLI `agentmux look` command mapped to relay `look`.
4. Add tests for bounds and response schema parity.
