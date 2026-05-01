## 1. Contract Design

- [x] 1.1 Add `grant` capability contract (vocabulary/default/invalid-scope
      rejection) to relay authorization model.
- [x] 1.2 Lock UI-only decision submitter class gate and non-spoofable decision
      actor identity rules.
- [x] 1.3 Lock same-bundle permission routing/decision boundary and rejection
      code for cross-bundle attempts.

## 2. Queue and Lifecycle Contract

- [x] 2.1 Add bounded queue contract (`max_pending`, override range, overflow
      code) and deterministic FIFO ordering.
- [x] 2.2 Lock queue durability, restart restoration behavior, and fail-fast
      corruption handling code.
- [x] 2.3 Lock replay/bootstrap parity behavior on authorized UI
      connect/reconnect (`permission.snapshot` then FIFO replay).

## 3. Pending Lifecycle and Enforcement Mapping

- [x] 3.1 Lock non-expiring pending lifecycle for permission requests in alpha
      (no auto-expiry timer).
- [ ] 3.2 Relock deterministic terminal reason taxonomy for
      selected/cancelled/already-resolved.
- [ ] 3.3 Relock deterministic mapping table from permission terminal outcomes to
      ACP action and sender-visible terminal outcome/reason_code.
- [ ] 3.4 Lock ACP-native decision contract:
      `permission.resolve` with outcomes `selected|cancelled`, explicit
      `option_id` required only for `selected`, and no relay fallback inference.

## 4. TUI Contract

- [x] 4.1 Add TUI pending permission visibility requirements using stable
      identifiers and metadata.
- [ ] 4.2 Relock TUI decision-action contract to ACP-native
      `permission.resolve` keyed by `permission_request_id`.
- [x] 4.3 Lock UI dedupe expectation for at-least-once replay.
- [ ] 4.4 Add session-scoped Look workflow requirements for permission
      decisioning (filtering, deterministic selection, key hints).
- [ ] 4.5 Add explicit permission-option selection requirements for TUI actions
      (`option_id` support).

## 5. Implementation Follow-up (post-approval)

- [x] 5.1 Implement relay queue persistence and lifecycle event emission.
- [x] 5.2 Implement `grant` policy evaluation for permission decisions.
- [ ] 5.3 Rework ACP permission resolution mapping to ACP-native
      `selected|cancelled` decision outcomes.
- [ ] 5.4 Rework TUI decision actions to use ACP-native `permission.resolve`
      (replace approve/deny abstraction).
- [ ] 5.5 Add integration/unit coverage for queue bounds, replay, class gate,
      pending lifecycle, and mapping behavior.
- [ ] 5.6 Implement relay event payload option metadata for explicit operator
      selection.
- [ ] 5.7 Implement TUI explicit option selection and session-scoped Look
      actions.
- [ ] 5.9 Remove relay-side approve/deny abstraction and option inference;
      enforce `permission.resolve` contract validation.
- [ ] 5.8 Add manual ACP permission-flow test plan for operator UX validation.

## 6. Validation

- [x] 6.1 Run `openspec validate add-acp-permission-request-ui-mediated --strict`.
