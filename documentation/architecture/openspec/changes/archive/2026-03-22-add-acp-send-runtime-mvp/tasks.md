## 1. Contract Updates

- [x] 1.1 Add ACP lifecycle selector precedence requirement with runtime
      persisted-session fallback.
- [x] 1.2 Lock fail-fast no-fallback semantics for ACP load path.
- [x] 1.3 Add ACP capability-gating requirement for initialize/load/prompt.
- [x] 1.4 Add transport-specific ACP timeout semantics (turn-wait mapping).
- [x] 1.5 Add ACP stop-reason and timeout outcome mapping requirement.

## 2. Implementation Follow-up

- [x] 2.1 Implement ACP session-id persistence ownership in runtime state.
- [x] 2.2 Implement lifecycle precedence using config + persisted state.
- [x] 2.3 Implement capability checks and stable error codes.
- [x] 2.4 Implement ACP turn outcome mapping with stable reason codes.

## 3. Tests

- [x] 3.1 Add integration tests for lifecycle precedence resolution.
- [x] 3.2 Add integration tests for load-failure no-fallback behavior.
- [x] 3.3 Add integration tests for capability-missing failure paths.
- [x] 3.4 Add integration tests for stop-reason/outcome mapping.
- [x] 3.5 Add integration tests for ACP turn timeout mapping.

## 4. Validation

- [x] 4.1 Run `openspec validate add-acp-send-runtime-mvp --strict`.
