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

- [x] 3.1 Lock non-expiring pending lifecycle for permission requests in MVP
      (no auto-expiry timer).
- [x] 3.2 Lock deterministic terminal reason taxonomy for
      approved/denied/cancelled/already-resolved.
- [x] 3.3 Lock deterministic mapping table from permission terminal outcomes to
      ACP action and sender-visible terminal outcome/reason_code.

## 4. TUI Contract

- [ ] 4.1 Add TUI pending permission visibility requirements using stable
      identifiers and metadata.
- [ ] 4.2 Add TUI approve/deny action contract keyed by
      `permission_request_id`.
- [ ] 4.3 Lock UI dedupe expectation for at-least-once replay.

## 5. Implementation Follow-up (post-approval)

- [x] 5.1 Implement relay queue persistence and lifecycle event emission.
- [x] 5.2 Implement `grant` policy evaluation for permission decisions.
- [x] 5.3 Implement ACP permission resolution mapping to transport actions.
- [ ] 5.4 Implement/adjust TUI pending permissions view and decision actions.
- [ ] 5.5 Add integration/unit coverage for queue bounds, replay, class gate,
      pending lifecycle, and mapping behavior.

## 6. Validation

- [x] 6.1 Run `openspec validate add-acp-permission-request-ui-mediated-mvp --strict`.
