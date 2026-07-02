# pty-terminal-protocols Specification

## Purpose

Per-coder configuration of the `TERM` environment variable for
children spawned by the Pty transport. The capability governs
the schema, validation, and transport-side semantics of the
`[coders.<id>.pty].term-protocol` field.

## ADDED Requirements

### Requirement: Per-coder TERM env var selection

The Pty transport SHALL honor a per-coder `term-protocol` field
in `[coders.<id>.pty]` that selects the literal value passed as
the `TERM` environment variable when the transport spawns the
child coder process. The default value SHALL be `xterm-256color`,
which preserves the existing behavior verbatim.

#### Scenario: Default term-protocol preserves today's behavior

- **WHEN** `[coders.<id>.pty]` omits `term-protocol`
- **THEN** the Pty transport sets `TERM=xterm-256color` for the
  spawned child
- **AND** the child's view of its terminal is unchanged from
  today's behavior

#### Scenario: Explicit term-protocol sets TERM accordingly

- **WHEN** `[coders.<id>.pty]` sets `term-protocol = "xterm-kitty"`
- **THEN** the Pty transport sets `TERM=xterm-kitty` for the
  spawned child
- **AND** agentmux makes no claim about child-side behavior
  beyond setting the env var; downstream TUI capability
  detection is the child's responsibility

### Requirement: Closed enum schema

The `term-protocol` field SHALL accept a closed enum of
well-known terminal type names. The supported values SHALL
include at least: `xterm-256color`, `xterm-kitty`, `alacritty`,
`foot`, `wezterm`, `screen-256color`. Each value SHALL map 1:1
to the literal TERM env-var string of the same name.

#### Scenario: Accept known term-protocol value

- **WHEN** `[coders.<id>.pty]` sets `term-protocol = "xterm-kitty"`
- **THEN** config load succeeds
- **AND** the validated config carries the `XtermKitty` variant

#### Scenario: Reject unknown term-protocol value

- **WHEN** `[coders.<id>.pty]` sets
  `term-protocol = "xterm-kittie"` (typo)
- **THEN** config load fails with a structured "unknown
  variant" deserialization error from serde's enum-variant
  deserializer (the value is not a valid `TermProtocol`
  variant, unrelated to `deny_unknown_fields` which guards
  field-name typos only)

### Requirement: No effect on non-Pty transports

The `term-protocol` field SHALL only affect the Pty transport.
Tmux and ACP transports SHALL be unaffected by this field's
presence or absence on a coder entry.

#### Scenario: term-protocol has no effect on Tmux coder

- **WHEN** `[coders.<id>.tmux]` is configured for a coder
- **AND** `[coders.<id>.pty]` is absent
- **THEN** the Tmux transport is constructed as today
- **AND** `term-protocol` is not consulted

### Requirement: COLORTERM out of scope

The `COLORTERM` environment variable SHALL continue to be set to
`truecolor` for all Pty-spawned children, regardless of the
`term-protocol` field. Configurability of `COLORTERM` is out of
scope for this capability.

#### Scenario: COLORTERM remains truecolor

- **WHEN** a Pty-spawned child starts
- **THEN** the child sees `COLORTERM=truecolor` in its environment
- **AND** this is independent of the `term-protocol` field