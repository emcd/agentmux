## ADDED Requirements
### Requirement: Send Timeout Override Flags by Transport

`agentmux send` SHALL support transport-scoped timeout override flags:

- `--quiescence-timeout-ms <MS>` for tmux delivery quiescence wait behavior
- `--acp-turn-timeout-ms <MS>` for ACP turn-wait behavior

The command SHALL reject requests that provide both flags in one invocation
with `validation_conflicting_timeout_fields`.

Transport-incompatible timeout flags SHALL fail fast with
`validation_invalid_timeout_field_for_transport`.

#### Scenario: Reject conflicting timeout flags

- **WHEN** an operator invokes `agentmux send` with both
  `--quiescence-timeout-ms` and `--acp-turn-timeout-ms`
- **THEN** invocation fails with `validation_conflicting_timeout_fields`

#### Scenario: Reject tmux timeout flag for ACP target

- **WHEN** `agentmux send` targets ACP-backed session
- **AND** operator provides `--quiescence-timeout-ms`
- **THEN** invocation fails with
  `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject ACP timeout flag for tmux target

- **WHEN** `agentmux send` targets tmux-backed session
- **AND** operator provides `--acp-turn-timeout-ms`
- **THEN** invocation fails with
  `validation_invalid_timeout_field_for_transport`

### Requirement: CLI ACP Sync Delivery-Phase Passthrough

For sync ACP sends, CLI SHALL preserve relay-authored early-success markers in
structured output.

When relay returns phase-1 acknowledgment, CLI JSON output SHALL include:

- `outcome = delivered`
- `details.delivery_phase = "accepted_in_progress"`
- unchanged `message_id` for request tracing

#### Scenario: Preserve relay delivery-phase details in CLI JSON output

- **WHEN** operator invokes `agentmux send --delivery-mode sync` to ACP target
- **AND** relay returns phase-1 acknowledgment marker
- **THEN** CLI JSON output includes
  `details.delivery_phase = "accepted_in_progress"`
- **AND** preserves the same `message_id`
