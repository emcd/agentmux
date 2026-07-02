## MODIFIED Requirements

### Requirement: Send Timeout Override Flags by Transport

`agentmux send` SHALL NOT expose any transport-scoped timeout
override flag in v1. v1 of ACP delivery and v1 of Tmux delivery
are fully config-only: the per-coder config keys
`[coders.<id>.acp].prime-timeout-ms` and
`[coders.<id>.tmux].prime-timeout-ms` are the only timeout
surfaces.

The pre-existing `--quiescence-timeout-ms` CLI flag was retired
by the `tmux-wedge-detection` proposal; the pre-existing
`--acp-turn-timeout-ms` CLI flag is retired by this proposal.
Both flags are rejected at the CLI parser as unknown. Alpha
defaults apply: the rejection is a generic unknown-flag error;
the parser is NOT required to name a replacement. Operators who
hit the rejection consult the changelog.

Transport-incompatible timeout flags SHALL fail fast with
`validation_invalid_timeout_field_for_transport`. With no
transport-scoped timeout override flags in v1, this validation
class is reserved for future per-call overrides (if/when a
transport-neutral `--prime-timeout-ms` is reintroduced — see
`design.md` Future Work).

#### Scenario: Reject retired tmux timeout flag

- **WHEN** `agentmux send` is invoked with
  `--quiescence-timeout-ms` (a flag that does not exist in v1)
- **THEN** invocation fails at the CLI parser as an unknown flag

#### Scenario: Reject retired ACP timeout flag

- **WHEN** `agentmux send` is invoked with
  `--acp-turn-timeout-ms` (a flag that does not exist in v1)
- **THEN** invocation fails at the CLI parser as an unknown flag

#### Scenario: Reject hypothetical ACP prime timeout flag

- **WHEN** `agentmux send` is invoked with
  `--acp-prime-timeout-ms` (a flag that has never existed)
- **THEN** invocation fails at the CLI parser as an unknown flag