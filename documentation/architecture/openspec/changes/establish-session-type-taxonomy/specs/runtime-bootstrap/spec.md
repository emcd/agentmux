## MODIFIED Requirements

### Requirement: TUI Sender Configuration Files

The runtime SHALL support global user session configuration at:

- normal config path: `<config-root>/users.toml`
- debug/testing override path:
  `.auxiliary/configuration/agentmux/overrides/users.toml`

Supported fields SHALL use kebab-case and include:

- `default-bundle` (optional)
- `default-session` (optional)
- `[[sessions]]` entries with:
  - required `id` (in `session@GLOBAL` canonical form)
  - required exactly one session-type subtable
    (`[sessions.ui]` for TUI operators; `[sessions.pubsub]` for embedded
    agents; other types permitted for future use)
  - optional `name`
  - optional `policy`

Missing files SHALL not be treated as errors.
Malformed files SHALL fail fast with structured bootstrap validation errors.
Session `id` SHALL be in `session@GLOBAL` canonical form and SHALL be unique
within the file.

#### Scenario: Resolve sender from session entry in global users.toml

- **WHEN** runtime selects session `user@GLOBAL`
- **AND** `[[sessions]]` in `users.toml` contains `id = "user@GLOBAL"`
- **THEN** runtime resolves sender identity as `user@GLOBAL`

#### Scenario: Reject unknown configured default session

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `users.toml`
- **THEN** startup fails with stable validation code

### Requirement: TUI Override File VCS Posture

Global users local testing override file SHALL follow the existing local
override VCS posture so per-user test defaults do not leak into shared tracked
configuration.

#### Scenario: Keep override users.toml under ignored overrides directory

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/agentmux/overrides/users.toml` is covered
  by the existing ignored overrides path

## ADDED Requirements

### Requirement: Session Type Validation in Config Load

The runtime SHALL validate session-type subtable presence and exclusivity at
config load time for both bundle `[[sessions]]` entries and global
`users.toml` entries:

- Exactly one session-type subtable (`tmux`, `acp`, `ui`, `pubsub`) SHALL be
  present per session entry.
- Zero or multiple subtables SHALL fail fast with a structured config error.
- Unrecognized subtable keys SHALL fail fast with a structured config error.

`ui` and `pubsub` session types with empty subtable bodies SHALL be valid at
parse time. Runtime MAY emit `runtime_session_type_not_implemented` at
startup for these types without treating the configuration itself as invalid.

#### Scenario: Reject session entry with no type subtable

- **WHEN** a `[[sessions]]` entry has no recognized type subtable
- **THEN** config load fails with a structured validation error

#### Scenario: Reject session entry with multiple type subtables

- **WHEN** a `[[sessions]]` entry declares both `[sessions.tmux]` and
  `[sessions.acp]`
- **THEN** config load fails with a structured validation error

#### Scenario: Accept ui session with empty subtable body

- **WHEN** a `[[sessions]]` entry declares `[sessions.ui]` with no additional
  fields
- **THEN** config load succeeds
