## MODIFIED Requirements

### Requirement: Send Timeout Override Flags by Transport

`agentmux send` SHALL support a transport-scoped ACP timeout override flag:

- `--acp-turn-timeout-ms <MS>` for ACP turn-wait behavior

Tmux delivery is configured per coder via the
`[coders.<id>.tmux].prime-timeout-ms` TOML key (see the session-relay
Tmux Prime Timeout requirement); `agentmux send` SHALL NOT expose a
per-call tmux timeout override flag. v1 of Tmux delivery is
config-only.

Transport-incompatible timeout flags SHALL fail fast with
`validation_invalid_timeout_field_for_transport`.

#### Scenario: Reject ACP timeout flag for tmux target

- **WHEN** `agentmux send` targets tmux-backed session
- **AND** operator provides `--acp-turn-timeout-ms`
- **THEN** invocation fails with
  `validation_invalid_timeout_field_for_transport`