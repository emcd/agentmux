## Context

ACP stdio transport is subprocess-bound. Closing stdin ends that ACP server
process, so per-request subprocess lifecycle is brittle under slow model
latencies. Existing send contracts are terminal-outcome-oriented and do not
separate delivery acknowledgment from turn completion for ACP.

## Goals

- Make ACP timeout semantics transport-appropriate and explicit.
- Preserve deterministic sync behavior with clear first-activity acknowledgment.
- Define a robust persistent ACP worker lifecycle with bounded queueing.
- Keep authorization centralized in relay, including ACP permission handling.

## Non-Goals

- Support ACP HTTP transport in this change.
- Introduce compatibility aliases for deprecated timeout fields.
- Redesign look semantics in this proposal.

## Decisions

- Decision: Timeout split
  - Use request field `acp_turn_timeout_ms` for ACP targets.
  - Use coder default `[coders.acp] turn-timeout-ms`.
  - Precedence:
    1. request `acp_turn_timeout_ms`
    2. coder `[coders.acp] turn-timeout-ms`
    3. system default `120000` ms
  - Reject transport-incompatible timeout fields with
    `validation_invalid_timeout_field_for_transport`.
  - Reject conflicting timeout fields with
    `validation_conflicting_timeout_fields`.

- Decision: Two-phase sync semantics for ACP
  - Sync response `outcome=delivered` for ACP means first activity observed,
    not terminal stopReason completion.
  - Relay marks this explicitly with
    `details.delivery_phase = "accepted_in_progress"`.
  - Terminal completion is internal worker lifecycle signal in MVP; it is not
    part of sender-facing `send` completion semantics.

- Decision: Internal ACP terminal-readiness state
  - Relay tracks ACP worker availability using terminal stopReason updates.
  - State model for MVP:
    - `available`: worker is healthy and ready to accept next prompt
    - `busy`: prompt accepted and turn still in progress
    - `unavailable`: transport/process failure requires worker restart
  - Transition rules:
    - first ACP activity observed -> `busy`
    - terminal stopReason observed -> `available`
    - disconnect/error before recovery -> `unavailable`
  - Sender-facing `send` surfaces are unchanged by this terminal state in MVP.

- Decision: Persistent ACP worker lifecycle
  - One worker per target session with serialized processing.
  - Fixed MVP queue bound: `max_pending = 64`.
  - Overflow returns `runtime_acp_queue_full`.
  - If disconnect occurs before first-activity ack, request fails with
    `runtime_acp_connection_closed`.
  - If disconnect occurs after first-activity ack, response is not mutated;
    worker transitions to `unavailable` and recovery is handled internally.
  - Restart sequence:
    1. spawn
    2. initialize
    3. select `session/load` or `session/new`
    4. prompt

- Decision: Stage failure taxonomy
  - `runtime_acp_initialize_failed`
  - `runtime_acp_session_load_failed`
  - `runtime_acp_session_new_failed`
  - `runtime_acp_prompt_failed`
  - `acp_turn_timeout`

- Decision: Permission handling and auth boundary
  - Relay policy is authoritative for ACP permission decisions.
  - Adapters do not implement shadow auth.
  - Policy denial uses canonical `authorization_forbidden` details minimum:
    `capability`, `requester_session`, `bundle_name`, `reason`.
  - Optional additive details may include:
    `target_session`/`targets`, `policy_rule_id`, `permission_kind`,
    `request_id`.
  - ACP permission infrastructure failures:
    `runtime_acp_permission_timeout`, `runtime_acp_permission_failed`.

## Risks / Trade-offs

- Two-phase sync semantics reduce false negatives for slow models, but require
  consumers to treat sync success as delivery-ack, not turn-finished.
- Persistent workers improve reliability but increase lifecycle complexity
  (queueing, reconnect ordering, teardown handling).

## Migration Plan

1. Add timeout field/schema updates across relay, MCP, and CLI.
2. Add two-phase sync behavior and internal readiness transitions.
3. Add persistent ACP workers and bounded queueing.
4. Add permission-request handling with relay-policy mapping.
