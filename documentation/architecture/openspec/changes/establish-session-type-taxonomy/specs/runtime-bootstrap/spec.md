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
  - required exactly one coder-less marker subtable: `[sessions.ui]` (TUI
    operators) or `[sessions.pubsub]` (embedded agents)
  - optional `name`
  - optional `policy`

Global user sessions are coder-less by construction; a `coder` reference is
not accepted in `users.toml` entries.

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

The runtime SHALL validate session shape exclusivity at config load time:

- A bundle `[[sessions]]` entry SHALL declare exactly one shape: a coder-backed
  shape (a flat `coder` reference, with optional `coder-session-id`) or a
  coder-less shape (exactly one `[sessions.ui]` or `[sessions.pubsub]` marker
  subtable).
- A global `users.toml` entry SHALL declare exactly one coder-less marker
  subtable; a `coder` reference is not accepted.
- Zero shapes, or more than one shape, SHALL fail fast with a structured
  config error.
- A `coder-session-id` on a coder-less session SHALL fail fast with a
  structured config error.
- Unrecognized subtable keys SHALL fail fast with a structured config error.

A coder-less `[sessions.ui]` or `[sessions.pubsub]` marker with an empty body
SHALL be valid at parse time. Runtime MAY emit
`runtime_session_type_not_implemented` at startup for these types without
treating the configuration itself as invalid.

#### Scenario: Reject session entry with neither coder nor marker

- **WHEN** a `[[sessions]]` entry declares no `coder` reference and no
  coder-less marker subtable
- **THEN** config load fails with a structured validation error

#### Scenario: Reject session entry declaring both coder and marker

- **WHEN** a `[[sessions]]` entry declares a `coder` reference and also a
  `[sessions.ui]` marker subtable
- **THEN** config load fails with a structured validation error

#### Scenario: Reject coder-session-id on coder-less session

- **WHEN** a `[[sessions]]` entry declares a `[sessions.ui]` marker and a
  `coder-session-id`
- **THEN** config load fails with a structured validation error

#### Scenario: Accept ui session with empty marker body

- **WHEN** a `[[sessions]]` entry declares `[sessions.ui]` with no additional
  fields
- **THEN** config load succeeds
