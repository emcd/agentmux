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

### Requirement: CLI raww command surface

CLI SHALL provide direct-write command:

`agentmux raww <target-session> --text <text> [--mode <normal|emergency>] [--no-enter] [--bundle <name>] [--as-session <id>] [--json]`

`<target-session>` SHALL be canonical session id.
`--no-enter` default SHALL be `false`.
`--mode` default SHALL be `normal`.

`--mode emergency` SHALL request the ordering break defined by the
`transport-contracts` capability's `Relay raww operation contract`: the write
overtakes that target's pending mail and bypasses the prompt-readiness gate. It
is supported on Tmux and Pty targets only, and CLI SHALL surface the relay's
rejection unchanged for any other transport.

Omitting `--mode` SHALL produce exactly the behavior an `agentmux raww`
invocation had before this change, so no existing operator command changes
meaning.

#### Scenario: Reject missing raww text

- **WHEN** operator invokes `agentmux raww` without `--text`
- **THEN** CLI rejects invocation with `validation_invalid_params`

#### Scenario: Map no-enter to no_enter true

- **WHEN** operator invokes `agentmux raww` with `--no-enter`
- **THEN** CLI forwards relay request with `no_enter = true`

#### Scenario: Default raww mode is normal

- **WHEN** operator invokes `agentmux raww` without `--mode`
- **THEN** CLI forwards relay request with `mode = "normal"`

#### Scenario: Forward emergency raww mode

- **WHEN** operator invokes `agentmux raww` with `--mode emergency`
- **THEN** CLI forwards relay request with `mode = "emergency"`

#### Scenario: Surface unsupported-transport rejection for emergency mode

- **WHEN** operator invokes `agentmux raww --mode emergency` against an ACP, UI,
  or Pubsub target
- **THEN** CLI surfaces the relay's `validation_invalid_params` unchanged
