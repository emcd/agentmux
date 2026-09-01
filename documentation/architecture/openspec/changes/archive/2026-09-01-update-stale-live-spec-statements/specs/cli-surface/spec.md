## MODIFIED Requirements

### Requirement: CLI raww actor identity resolution

CLI raww acting identity SHALL follow global TUI-session selector contract:
- explicit `--as-session`
- otherwise `default-session` from `users.toml`

CLI SHALL NOT use repository association fallback for raww actor identity.

#### Scenario: Reject unknown as-session selector for raww

- **WHEN** operator passes unknown `--as-session` for `agentmux raww`
- **THEN** CLI rejects invocation with `validation_unknown_sender`
