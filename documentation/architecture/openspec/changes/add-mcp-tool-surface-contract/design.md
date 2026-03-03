## Context

The `session-relay` MVP proposal defines relay behavior and tmux lifecycle
semantics. This change defines the public MCP contract that clients use to
invoke those behaviors.

The design goal is an MVP-minimal tool surface that is easy for LLMs to use
safely and easy for humans to inspect.

## Goals / Non-Goals

- Goals:
  - Provide stable MCP contracts for recipient discovery and chat delivery.
  - Support explicit target list delivery and full-bundle broadcast delivery.
  - Provide deterministic per-target delivery outcomes and machine-readable
    errors.
- Non-Goals:
  - MCP-based bundle configuration and reconciliation controls.
  - Streaming or subscription-based delivery APIs.
  - Accept/done acknowledgement protocols.
  - Cross-host transport and authentication expansion.

## Decisions

- Decision: expose two MCP tools for MVP.
  - Tools:
    - `list`
    - `chat`
  - Rationale: this is sufficient for MVP workflows when bundles are manually
    configured by the operator.

- Decision: bundle configuration is out-of-band for MVP.
  - Rationale: keeps MCP surface narrow while relay behavior is stabilized.
  - Consequence: MCP clients can read and use configured bundles, but do not
    mutate them in MVP.

- Decision: `chat` uses exactly one targeting mode.
  - Modes:
    - `targets` (non-empty list)
    - `broadcast` (boolean true)
  - Rationale: a list of one target naturally covers single-recipient delivery
    and avoids redundant one-target schema branches.

- Decision: sender identity is inferred from MCP server association.
  - Rationale: callers should not spoof or manually re-specify sender identity
    in every `chat` request.
  - Consequence: each MCP server instance maps to one configured sender session
    identity.

- Decision: recipients may expose optional display names.
  - Rationale: stable routing identifiers and human-readable names serve
    different goals.
  - Consequence: `session_name` remains canonical for routing while
    `display_name` is optional metadata for humans.

- Decision: delivery responses are synchronous and per-target.
  - Response includes aggregate status and `results[]` entries with target
    outcome details.
  - Rationale: callers can make immediate routing decisions without waiting for
    separate callbacks.

- Decision: errors use stable code-first objects.
  - Error shape:
    - `code`
    - `message`
    - `details` (optional)
  - Rationale: stable codes support robust client automation while messages stay
    human-readable.

- Decision: schemas are versioned in responses.
  - Responses include `schema_version` and `request_id` when provided.
  - Rationale: future evolution can remain backward-compatible.

## Error Code Set (MVP)

- `validation_invalid_arguments`
- `validation_unknown_bundle`
- `validation_unknown_recipient`
- `validation_unknown_sender`
- `validation_conflicting_targets`
- `validation_empty_targets`
- `delivery_quiescence_timeout`
- `transport_tmux_failure`
- `internal_unexpected_failure`

## Risks / Trade-offs

- Compact tool count keeps adoption simple but concentrates responsibility in
  `chat`.
- Strict target-mode validation can feel rigid for ad-hoc callers but prevents
  high-impact mistakes.

## Migration Plan

1. Implement tool registration and schemas in the MCP server.
2. Add request validation and deterministic error mapping.
3. Add integration tests for list and chat target modes, including
   partial-delivery outcomes.
4. Document tool contracts for agent and human users.

## Open Questions

- None for MVP. MCP startup uses connect-only relay checks and returns a
  remediation error when relay is unavailable.
