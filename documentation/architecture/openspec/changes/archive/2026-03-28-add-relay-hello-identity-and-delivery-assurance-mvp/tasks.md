## 1. Contract Design

- [x] 1.1 Modify hello-claim contract to reject duplicate live identity claims with `runtime_identity_claim_conflict`.
- [x] 1.2 Lock deterministic MVP dead-owner rule as hard-dead evidence only (no active probe in claim path).
- [x] 1.3 Modify reconnect contract to remove implicit latest-claim-wins behavior and align to conflict rejection semantics.
- [x] 1.4 Lock transport/recipient matrix for stream delivery assurance so stream-specific rules do not alter tmux/ACP behavior.
- [x] 1.5 Lock canonical machine-completion update schema keyed by `message_id` with phase/outcome semantics and unchanged external terminal vocabulary.

## 2. Implementation Follow-up (post-approval)

- [x] 2.1 Update relay stream registry claim path to enforce single-owner semantics and conflict rejection details.
- [x] 2.2 Update reconnect handling to tolerate conflict responses and retry only after stale owner clears.
- [x] 2.3 Emit deterministic stream completion updates for routed/delivered/failed transitions.
- [x] 2.4 Preserve disconnected-ui queue/retry behavior while adding stale-binding cleanup on write failure.
- [x] 2.5 Add tests for duplicate live claims, hard-dead replacement, and completion update payload parity.

## 3. Validation

- [x] 3.1 Run `openspec validate add-relay-hello-identity-and-delivery-assurance-mvp --strict`.
