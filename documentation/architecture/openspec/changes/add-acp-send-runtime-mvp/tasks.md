## 1. Contract Updates

- [ ] 1.1 Add ACP lifecycle selector precedence requirement with runtime
      persisted-session fallback.
- [ ] 1.2 Lock fail-fast no-fallback semantics for ACP load path.
- [ ] 1.3 Add ACP capability-gating requirement for initialize/load/prompt.
- [ ] 1.4 Add transport-specific ACP timeout semantics (turn-wait mapping).
- [ ] 1.5 Add ACP stop-reason and timeout outcome mapping requirement.

## 2. Implementation Follow-up

- [ ] 2.1 Implement ACP session-id persistence ownership in runtime state.
- [ ] 2.2 Implement lifecycle precedence using config + persisted state.
- [ ] 2.3 Implement capability checks and stable error codes.
- [ ] 2.4 Implement ACP turn outcome mapping with stable reason codes.

## 3. Tests

- [ ] 3.1 Add integration tests for lifecycle precedence resolution.
- [ ] 3.2 Add integration tests for load-failure no-fallback behavior.
- [ ] 3.3 Add integration tests for capability-missing failure paths.
- [ ] 3.4 Add integration tests for stop-reason/outcome mapping.
- [ ] 3.5 Add integration tests for ACP turn timeout mapping.

## 4. Validation

- [ ] 4.1 Run `openspec validate add-acp-send-runtime-mvp --strict`.
