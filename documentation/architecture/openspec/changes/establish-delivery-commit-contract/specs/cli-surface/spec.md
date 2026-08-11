## MODIFIED Requirements

### Requirement: Send Timeout Override Flags by Transport

`agentmux send` SHALL NOT expose any transport-scoped timeout override flag.
Delivery patience is relay configuration, not a per-call or per-coder surface:
the `[delivery]` keys in `relay.toml` (see the `runtime-bootstrap` capability's
`Relay Configuration File` requirement) are the only timeout surfaces.

This change deletes every per-coder timeout key that this requirement previously
enumerated — `[coders.<id>.acp].prime-timeout-ms`,
`[coders.<id>.pty].prime-timeout-ms`, `[coders.<id>.tmux].prime-timeout-ms`, and
`[coders.<id>.tmux].readiness-timeout-ms` — because how long a delivery may wait
is a property of the relay's patience rather than of any coder.

Adding a delivery-patience key SHALL NOT be read as licence to add a per-call
override for it. The no-per-call-override property is the invariant this
requirement states; the enumeration of keys is incidental to it and SHALL be kept
current, so that "the only timeout surfaces" remains a true statement rather than
a stale one. The enumeration SHALL be reconciled against the authoritative
descriptor lists in the `addressing-routing` capability's `Bundle Membership
Configuration` requirement and the `runtime-bootstrap` capability's `Relay
Configuration File` requirement, rather than extended only with the key a change
happens to introduce.

Transport-incompatible timeout flags SHALL fail fast with
`validation_invalid_timeout_field_for_transport`. With no transport-scoped
timeout override flags, this validation class is reserved for a future per-call
override, if one is ever reintroduced.

#### Scenario: Reject retired tmux timeout flag

- **WHEN** `agentmux send` is invoked with `--quiescence-timeout-ms` (a flag that
  does not exist)
- **THEN** invocation fails at the CLI parser as an unknown flag

#### Scenario: Reject retired ACP timeout flag

- **WHEN** `agentmux send` is invoked with `--acp-turn-timeout-ms` (a flag that
  does not exist)
- **THEN** invocation fails at the CLI parser as an unknown flag

#### Scenario: No flag bounds how long a delivery waits

- **WHEN** an operator wants to change how long a delivery waits for a target to
  become ready
- **THEN** no CLI flag and no configuration key offers that control, because the
  wait is unbounded by design
- **AND** `agentmux send` exposes no per-call timeout override of any kind
