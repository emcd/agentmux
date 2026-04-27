## Context

The current runtime has clear contracts for:
- envelope delivery (`send`),
- inspection (`look`),
- list/help/about surfaces,
- association-derived actor identity and canonical authorization-denial schema.

A raw direct-write path is needed for operators and TUI direct interaction
without introducing target-resolution drift, sender spoofing, or transport-
specific behavior divergence.
It is also required for slash-command style interactions that are not
expressible through envelope-based `send` delivery.

## Goals

- Add one deterministic raw direct-write operation across relay/CLI/MCP/TUI.
- Preserve canonical target/error taxonomy.
- Keep authorization relay-authoritative and non-spoofable.
- Reuse existing ACP worker path (no new ACP capability surface).
- Keep response acceptance-oriented and deterministic.

## Non-Goals

- Broadcast raw writes.
- Cross-bundle raww in MVP.
- UI-session targets as raww recipients.
- Terminal completion guarantee in raww immediate response.

## Decisions

1. Operation and target scope
- Operation name is `raww`.
- Exactly one explicit canonical target session id per request.
- Same-bundle only in MVP.

2. Validation/error taxonomy
- Unknown/non-canonical target: `validation_unknown_target`.
- Cross-bundle attempt: `validation_cross_bundle_unsupported`.
- Invalid params and unsupported target class: `validation_invalid_params`.

3. Authorization mapping
- New policy control key: `raww`.
- MVP allowed scopes: `none`, `self`, `all:home`.
- Denial capability label: `raww.write`.
- Canonical `authorization_forbidden` details minimum remains unchanged.

4. Sender authority (MCP)
- MCP raww actor identity is association-derived only.
- Caller-supplied sender-like fields are rejected with
  `validation_invalid_params`.

5. Transport mapping
- tmux target: inject literal text and append Enter by default.
- optional opt-out flag maps to `no_enter=true`.
- acp target: submit via existing shared worker/client `session/prompt` path.
- No new ACP capability is required.

6. Response contract
- Acceptance-oriented success only.
- Required success fields: `status`, `target_session`, `transport`.
- Optional success fields: `request_id`, `message_id`, `details`.
- Accepted success status value: `accepted`.
- ACP accepted success includes
  `details.delivery_phase = "accepted_in_progress"`.

7. Input bounds and safety
- UTF-8 multiline text allowed.
- Payloads larger than 32 KiB UTF-8 bytes fail with
  `validation_invalid_params`.
- Relay treats input as opaque text and does not evaluate/expand it.

## Risks / Trade-offs

- New policy control introduces config-surface expansion.
  - Mitigation: conservative default and strict scope validation.
- ACP raww acceptance before terminal completion can surprise callers.
  - Mitigation: explicit acceptance-phase contract and delivery phase details.
- UI-target ambiguity in future extension.
  - Mitigation: explicit MVP unsupported-target-class rejection.

## Migration Notes

- Existing policies without `raww` remain conservative by default.
- No breaking changes are required for existing `send`/`look` operations.
