## ADDED Requirements

### Requirement: CLI raww command surface

CLI SHALL provide direct-write command:

`agentmux raww <target-session> --text <text> [--no-enter] [--bundle <name>] [--as-session <id>] [--json]`

`<target-session>` SHALL be canonical session id.
`--no-enter` default SHALL be `false`.

#### Scenario: Reject missing raww text

- **WHEN** operator invokes `agentmux raww` without `--text`
- **THEN** CLI rejects invocation with `validation_invalid_params`

#### Scenario: Map no-enter to no_enter true

- **WHEN** operator invokes `agentmux raww` with `--no-enter`
- **THEN** CLI forwards relay request with `no_enter = true`

### Requirement: CLI raww actor identity resolution

CLI raww acting identity SHALL follow global TUI-session selector contract:
- explicit `--as-session`
- otherwise configured default session in `tui.toml`

CLI SHALL NOT use repository association fallback for raww actor identity.

#### Scenario: Reject unknown as-session selector for raww

- **WHEN** operator passes unknown `--as-session` for `agentmux raww`
- **THEN** CLI rejects invocation with `validation_unknown_sender`

### Requirement: CLI raww relay taxonomy passthrough

CLI raww SHALL surface canonical relay validation/authorization codes unchanged,
including:
- `validation_unknown_target`
- `validation_cross_bundle_unsupported`
- `validation_invalid_params`
- `authorization_forbidden`

#### Scenario: Surface unknown target code for raww

- **WHEN** relay returns `validation_unknown_target` for raww
- **THEN** CLI surfaces `validation_unknown_target`

### Requirement: CLI raww machine output contract

When `--json` is requested, CLI raww successful output SHALL include required
fields:
- `status`
- `target_session`
- `transport`

CLI MAY include optional fields:
- `request_id`
- `message_id`
- `details`

For ACP accepted success, CLI SHALL preserve
`details.delivery_phase = "accepted_in_progress"`.

#### Scenario: Preserve accepted_in_progress detail in json output

- **WHEN** relay raww success includes
  `details.delivery_phase = "accepted_in_progress"`
- **THEN** CLI `--json` output includes same `details.delivery_phase`
