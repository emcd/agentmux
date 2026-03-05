## 1. Implementation

- [x] 1.1 Add optional prompt-readiness template fields to bundle member
      configuration.
- [x] 1.2 Validate prompt-readiness regex during configuration loading.
- [x] 1.3 Apply prompt-readiness gating in relay delivery loop after
      quiescence is satisfied.
- [x] 1.4 Return distinct timeout reason when prompt-readiness does not match
      before delivery timeout.
- [x] 1.5 Add tests for valid and invalid prompt-readiness templates.
- [x] 1.6 Add integration tests for delivery success and timeout with
      prompt-readiness templates.
- [x] 1.7 Document prompt-readiness template configuration and behavior after
      design review approval.

## 2. Validation

- [x] 2.1 Run `cargo check --all-targets --all-features`.
- [x] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 2.3 Run `cargo test --all-features`.
- [x] 2.4 Run `openspec validate add-prompt-readiness-gating --strict`.
