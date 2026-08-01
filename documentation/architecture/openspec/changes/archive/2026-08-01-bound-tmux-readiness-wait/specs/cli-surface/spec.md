## MODIFIED Requirements

### Requirement: Send Timeout Override Flags by Transport

`agentmux send` SHALL NOT expose any transport-scoped timeout
override flag in v1. v1 of ACP, Tmux, and Pty delivery is fully
config-only: the per-coder config keys
`[coders.<id>.acp].prime-timeout-ms`,
`[coders.<id>.pty].prime-timeout-ms`,
`[coders.<id>.tmux].prime-timeout-ms`, and
`[coders.<id>.tmux].readiness-timeout-ms` are the only timeout
surfaces.

Adding a per-coder timeout key SHALL NOT be read as licence to add a per-call
override for it. The config-only property is the invariant this requirement
states; the enumeration of keys is incidental to it and SHALL be kept current as
keys are added, so that "the only timeout surfaces" remains a true statement
rather than a stale one. The enumeration SHALL be reconciled against the
authoritative descriptor list in the `addressing-routing` capability's `Bundle
Membership Configuration` requirement rather than extended only with the key a
change happens to introduce. Reconciling it here restored
`[coders.<id>.pty].prime-timeout-ms`, which shipped with `add-pty-transport` and
was never added to this list.

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

#### Scenario: The readiness bound has no per-call flag

- **WHEN** an operator wants to change how long a Tmux delivery waits for a
  target to become ready
- **THEN** the only surface is the per-coder
  `[coders.<id>.tmux].readiness-timeout-ms` config key
- **AND** `agentmux send` exposes no flag to override it for one call
