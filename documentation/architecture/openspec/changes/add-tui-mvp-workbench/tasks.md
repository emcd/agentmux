## 1. Proposal + Design

- [ ] 1.1 Confirm `tui-surface` MVP boundaries and explicit non-goals.
- [ ] 1.2 Lock view model and navigation skeleton:
      - recipients view
      - compose view
      - look snapshot view
      - delivery feedback view
- [ ] 1.3 Lock recipient interaction model:
      - explicit `To`/`Cc` fields
      - deterministic selected-recipient ID state
      - `Tab` completion behavior
      - recipient-picker overlay shortcut behavior
- [ ] 1.4 Lock target identifier grammar and MVP validation semantics:
      - `<session-id>` accepted in MVP
      - `<bundle-id>/<session-id>` reserved for forward compatibility
      - cross-bundle behavior remains unsupported in MVP
- [ ] 1.5 Map each TUI interaction to existing relay contracts and error codes
      (`list`, `send`, `look` + stable validation taxonomy).

## 2. Acceptance + Test Planning

- [ ] 2.1 Define acceptance scenarios for recipient discovery/selection.
- [ ] 2.2 Define acceptance scenarios for compose-and-send outcomes
      (`async` and `sync` delivery modes).
- [ ] 2.3 Define acceptance scenarios for look snapshot rendering.
- [ ] 2.4 Define failure-path scenarios for:
      - invalid recipient identifiers,
      - unsupported cross-bundle scope,
      - relay-unavailable and validation errors.
- [ ] 2.5 Define explicit guardrails to prevent non-goal scope creep during
      MVP implementation.

## 3. Review + Lock

- [ ] 3.1 Review first-pass OpenSpec with `master`, `relay`, and `mcp`.
- [ ] 3.2 Incorporate review feedback and lock final MVP scope.
- [ ] 3.3 Open implementation lane only after coordinator/human approval.

## 4. Validation

- [ ] 4.1 Run `openspec validate add-tui-mvp-workbench --strict`.
