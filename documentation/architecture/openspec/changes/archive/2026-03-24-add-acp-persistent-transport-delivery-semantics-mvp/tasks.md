## 1. Implementation

- [x] 1.1 Add ACP timeout contract fields and validation:
      - relay request field `acp_turn_timeout_ms`
      - coder default field `[coders.acp] turn-timeout-ms`
      - precedence: request > coder default > system default
      - reject transport-incompatible timeout fields with canonical codes
      - reject conflicting timeout fields (`validation_conflicting_timeout_fields`)
- [x] 1.2 Implement two-phase ACP sync send semantics:
      - mark early success on first ACP activity
      - return `details.delivery_phase = accepted_in_progress`
      - preserve existing status aggregation behavior
- [x] 1.3 Implement internal ACP readiness-state transitions:
      - set worker `busy` when first ACP activity is observed
      - set worker `available` when terminal stopReason is observed
      - set worker `unavailable` on disconnect/error requiring restart
      - keep sender-facing `send` response contract unchanged
- [x] 1.4 Implement persistent ACP worker lifecycle:
      - one worker per target session
      - serialized queue with fixed `max_pending = 64`
      - overflow handling (`runtime_acp_queue_full`)
      - reconnect/restart sequence and failure taxonomy
      - shared worker path for ACP sync + async sends
- [x] 1.5 Implement ACP permission-request readiness signaling (MVP):
      - handle `session/request_permission` in ACP loop as activity
      - treat permission-request state as non-ready (`busy`) until turn completion
      - defer policy-driven allow/deny mapping to follow-up delta (`todos/acp/2`)
- [x] 1.6 Update CLI `send` surface to include ACP timeout override flag and
      transport-specific validation behavior.
- [x] 1.7 Update MCP `send` surface to include ACP timeout override field and
      transport-specific validation behavior.

## 2. Testing

- [x] 2.1 Unit tests for timeout precedence and field-validation failures.
- [x] 2.2 Integration tests for sync ACP first-activity acknowledgment semantics.
- [x] 2.3 Integration tests for ACP worker readiness-state transitions
      (`available` <-> `busy` and failure to `unavailable`).
- [x] 2.4 Integration tests for persistent worker queue bound and overflow code.
- [x] 2.5 Integration tests for worker disconnect/restart behavior before and
      after first-activity acknowledgment.
- [x] 2.6 Integration tests for ACP `session/request_permission` activity
      and non-ready readiness behavior.

## 3. Validation

- [x] 3.1 Run `openspec validate add-acp-persistent-transport-delivery-semantics-mvp --strict`.
- [x] 3.2 Run `cargo check --all-targets --all-features`.
- [x] 3.3 Run ACP-focused integration suite for send/relay/MCP transport behavior.
