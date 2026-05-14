## Context

ACP permission requests are transport-level pauses in an in-flight turn.
For alpha, relay must remain the sole authorization authority while enabling
operator decisioning through UI clients without introducing adapter-side policy
logic or sender identity spoofing.

Existing ACP send contracts already lock:

- turn-timeout field precedence (`acp_turn_timeout_ms` -> coder default ->
  system default),
- sync phase-1 delivery semantics (`accepted_in_progress`),
- terminal stop-reason mapping and worker readiness behavior.

This change layers deterministic permission handling on top of those contracts.

Normative external reference:
- ACP Tool Calls: https://agentclientprotocol.com/protocol/tool-calls.md

Implementation guidance:
- Implementers MUST treat ACP Tool Calls as the source-of-truth for
  `session/request_permission`, including the outcome contract
  (`selected` + `optionId`, `cancelled`) and the permission option kind
  vocabulary (`allow_once`, `allow_always`, `reject_once`, `reject_always`).

## Goals

- Keep relay authorization and enforcement authoritative.
- Provide deterministic, machine-consumable permission lifecycle signals.
- Prevent decision-actor spoofing.
- Bound queue memory and lock overflow behavior.
- Keep alpha same-bundle and fail-fast.

## Non-Goals

- Cross-bundle permission decisioning.
- Multi-party voting/consensus approval flows.
- Adapter-owned authorization rules.
- New sender-facing sync response shapes beyond current send contract.

## Decisions

1. Introduce `grant` policy capability in relay policy controls.
   - Allowed values (alpha scope): `none`, `all:home`.
   - Default when omitted: `none`.
   - Invalid values (`self`, `all:all`, unknown) fail with
     `validation_invalid_policy_scope`.

2. Lock UI-only decision submitter gate.
   - Permission decision actions require associated principal
     `client_class=ui`.
   - Non-UI decision attempts fail with
     `validation_invalid_client_class_for_action`.

3. Keep decision actor identity association-derived.
   - Payload identity fields (for example `ui_session_id`) are disallowed and
     fail with `validation_invalid_params`.

4. Use bounded, durable permission queue.
   - Bundle-scoped global FIFO by monotonic enqueue `sequence`.
   - `max_pending` default `256`, optional override
     `[relay.permission] max-pending` in `1..4096`.
   - Overflow fails with `runtime_permission_queue_full`.
   - Pending queue persists across restart; unrecoverable state fails fast with
     `runtime_permission_queue_unavailable`.

5. Use non-expiring pending semantics for permission requests in alpha.
   - Pending permission requests do not auto-expire.
   - Requests remain pending until explicit operator decision or hard terminal
     conditions (session/worker termination, queue state unrecoverable).
   - ACP send turn-timeout semantics remain unchanged and independent from
     permission decision lifecycle.

6. Preserve ACP permission option fidelity end-to-end.
   - Relay lifecycle payloads include ACP option metadata for operator choice.
   - Decision actions use ACP-native outcomes:
     - `selected` with required explicit `option_id`
     - `cancelled` without `option_id`
   - Relay MUST NOT infer or auto-select option IDs.

7. Lock canonical machine lifecycle carrier.
   - Relay stream events are authoritative machine channel:
     - `permission.snapshot` (bootstrap parity on UI connect/reconnect),
     - `permission.requested`,
     - `permission.resolved`.
   - Required correlation keys: `message_id`, `permission_request_id`.
   - Inscriptions are additive only.

8. Lock deterministic enforcement mapping.
   - Permission outcome maps to ACP selected/cancelled and sender-visible
     terminal outcome/reason_code with no ambiguity.
   - Sync phase-1 immutability remains unchanged.

## Risks / Trade-offs

- Added queue persistence and lifecycle event complexity in relay.
  - Mitigation: strict required fields, stable reason taxonomy, bounded queue.
- UI reconnect replay may duplicate events.
  - Mitigation: require UI dedupe by `permission_request_id`.

## Migration Notes

- Existing policies that do not define `grant` remain conservative by default
  (`none`).
- Existing send contracts remain valid; this change adds permission lifecycle
  handling inside ACP flow.
