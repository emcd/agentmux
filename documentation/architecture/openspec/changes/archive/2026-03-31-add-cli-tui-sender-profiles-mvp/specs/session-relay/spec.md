## MODIFIED Requirements

### Requirement: Hello Registration Contract

Each client stream SHALL begin with `hello` registration frame containing:

- `bundle_name`
- `session_id`
- `client_class` (`agent` | `ui`)

`hello` identity SHALL bind principal/session for that stream using canonical
identity key:

- `(bundle_name, session_id)`

For `client_class=agent`, `session_id` SHALL resolve via bundle
`[[sessions]]` configuration.

For `client_class=ui`, `session_id` SHALL resolve via global TUI sessions from
`<config-root>/tui.toml`.

If a second stream attempts `hello` for the same identity while the current
owner is still live, relay SHALL reject second claim with
`runtime_identity_claim_conflict`.

#### Scenario: Accept hello for configured UI session identity

- **WHEN** TUI client sends valid `hello` with `client_class=ui`
- **AND** `session_id` maps to configured global TUI session `id`
- **THEN** relay accepts hello and binds stream owner identity

#### Scenario: Reject hello for unknown UI session identity

- **WHEN** a stream sends `hello` with `client_class=ui`
- **AND** `session_id` is not present in global TUI sessions
- **THEN** relay rejects hello with `validation_unknown_sender`

## ADDED Requirements

### Requirement: UI Request-Path Sender Validation

Relay SHALL validate non-hello request-path UI sender identities using global
TUI sessions from `<config-root>/tui.toml`.

For request-path operations such as `send`, relay SHALL:

1. validate sender `session_id` exists in global TUI sessions,
2. evaluate authorization using that TUI session's `policy` reference,
3. return canonical `authorization_forbidden` when policy denies.

#### Scenario: Authorize send using global UI session policy

- **WHEN** relay receives `send` request with UI sender `session_id = "user"`
- **AND** global TUI sessions include `id = "user"` with `policy = "ui-default"`
- **THEN** relay evaluates authorization using policy `ui-default`

#### Scenario: Reject request-path sender missing from global UI sessions

- **WHEN** relay receives `send` request with UI sender `session_id = "ghost"`
- **AND** no global TUI session maps to `id = "ghost"`
- **THEN** relay rejects request with `validation_unknown_sender`
