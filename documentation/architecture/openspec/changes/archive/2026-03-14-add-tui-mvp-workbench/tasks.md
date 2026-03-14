## 1. Proposal + Design

- [x] 1.1 Confirm `tui-surface` MVP boundaries and explicit non-goals.
- [x] 1.2 Lock view model and navigation skeleton:
      - recipients view
      - compose view
      - look snapshot view
      - delivery feedback view
- [x] 1.3 Lock recipient interaction model:
      - explicit `To` field
      - deterministic selected-recipient ID state
      - conditional `Tab` behavior (completion in `To`, focus fallback otherwise)
      - `Enter` accepts active completion in `To`
      - `@`-prefixed immediate proposal behavior
      - recipient-picker overlay shortcut behavior
- [x] 1.4 Lock async-only delivery behavior in TUI MVP:
      - no delivery-mode toggle
      - events overlay shortcut behavior
      - pending indicator contract
- [x] 1.5 Lock target identifier grammar and MVP validation semantics:
      - `<session-id>` accepted in MVP
      - `<bundle-id>/<session-id>` reserved for forward compatibility
      - cross-bundle behavior remains unsupported in MVP
- [x] 1.6 Map each TUI interaction to existing relay contracts and error codes
      (`list`, `send`, `look` + stable validation taxonomy).

## 2. Acceptance + Test Planning

- [x] 2.1 Define acceptance scenarios for recipient discovery/selection.
- [x] 2.2 Define acceptance scenarios for compose-and-send outcomes
      (async-only delivery mode + pending/event surfaces).
- [x] 2.3 Define acceptance scenarios for look snapshot rendering.
- [x] 2.4 Define failure-path scenarios for:
      - invalid recipient identifiers,
      - unsupported cross-bundle scope,
      - relay-unavailable and validation errors.
- [x] 2.5 Define explicit guardrails to prevent non-goal scope creep during
      MVP implementation.

## 3. Review + Lock

- [x] 3.1 Review first-pass OpenSpec with `master`, `relay`, and `mcp`.
- [x] 3.2 Incorporate review feedback and lock final MVP scope.
- [x] 3.3 Open implementation lane only after coordinator/human approval.

## 4. Validation

- [x] 4.1 Run `openspec validate add-tui-mvp-workbench --strict`.
