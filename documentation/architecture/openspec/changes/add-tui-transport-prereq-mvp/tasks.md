## 1. Contract Design

- [ ] 1.1 Lock TUI sender identity precedence (`--sender` > `tui.toml` > association > fail-fast).
- [ ] 1.2 Lock relay->TUI structured event contract for incoming messages and delivery outcomes.
- [ ] 1.3 Lock ack/outcome mapping for TUI state model (`accepted|success|timeout|failed`).
- [ ] 1.4 Lock reconnect and transport error semantics with fail-fast behavior.
- [ ] 1.5 Lock same-bundle-only MVP scope for transport/history behavior.

## 2. Runtime/Config Design

- [ ] 2.1 Define `<config-root>/tui.toml` sender field contract.
- [ ] 2.2 Define startup precedence and validation behavior for unresolved sender identity.
- [ ] 2.3 Define interaction boundaries with existing runtime association resolution.

## 3. Implementation Follow-up (post-approval)

- [ ] 3.1 Add runtime parsing for `tui.toml` sender default.
- [ ] 3.2 Add relay event retrieval operation with canonical event payload schema.
- [ ] 3.3 Wire TUI runtime to consume structured relay events for delivery/history updates.
- [ ] 3.4 Add integration tests for sender precedence and fail-fast transport behaviors.

## 4. Validation

- [ ] 4.1 Run `openspec validate add-tui-transport-prereq-mvp --strict`.
