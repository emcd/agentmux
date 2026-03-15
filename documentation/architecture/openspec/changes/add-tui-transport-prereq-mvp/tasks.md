## 1. Contract Design

- [ ] 1.1 Lock TUI sender identity precedence (`--sender` > local testing override > `<config-root>/tui.toml` > association > fail-fast).
- [ ] 1.2 Lock delivery-state mapping for TUI state model (`accepted|success|timeout|failed`).
- [ ] 1.3 Lock reconnect and transport error semantics with fail-fast behavior.
- [ ] 1.4 Lock same-bundle-only MVP scope for transport/history behavior.
- [ ] 1.5 Lock CLI `agentmux tui --sender` contract in `cli-surface`.
- [ ] 1.6 Record dependency on `add-relay-stream-hello-transport-mvp` for relay stream protocol details.
- [ ] 1.7 Lock bare `agentmux` dispatch behavior (`TTY => tui`, `non-TTY => help + non-zero`).

## 2. Runtime/Config Design

- [ ] 2.1 Define `<config-root>/tui.toml` sender field contract for normal release/runtime use.
- [ ] 2.2 Define debug/testing-only local override sender contract.
- [ ] 2.3 Define startup precedence and validation behavior for unresolved sender identity.

## 3. Implementation Follow-up (post-approval)

- [ ] 3.1 Add runtime parsing for `<config-root>/tui.toml` sender default.
- [ ] 3.2 Add optional debug/testing local override support for TUI sender.
- [ ] 3.3 Wire TUI state/history mapping to relay push events from adjacent transport contract.
- [ ] 3.4 Add integration tests for sender precedence and fail-fast transport behaviors.

## 4. Validation

- [ ] 4.1 Run `openspec validate add-tui-transport-prereq-mvp --strict`.
