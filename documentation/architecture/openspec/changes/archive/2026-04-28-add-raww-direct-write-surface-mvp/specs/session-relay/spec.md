## ADDED Requirements

### Requirement: Relay raww operation contract

Relay SHALL expose a raw direct-write operation named `raww` for a single
explicit target session.

Request contract (MVP):
- `target_session` (required)
- `text` (required UTF-8 string)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)
- optional bundle selector with same-bundle-only enforcement

`raww` SHALL NOT support broadcast in MVP.

#### Scenario: Reject raww broadcast shape

- **WHEN** caller attempts to invoke `raww` without one explicit
  `target_session`
- **THEN** relay rejects the request with `validation_invalid_params`

### Requirement: Relay raww target resolution and bundle boundary

Relay raww target resolution SHALL use canonical session id identifiers only.

Validation behavior:
- unknown/non-canonical target -> `validation_unknown_target`
- explicit cross-bundle request in MVP -> `validation_cross_bundle_unsupported`

Validation precedence SHALL evaluate target/bundle constraints before
authorization policy checks.

#### Scenario: Reject unknown raww target

- **WHEN** caller invokes `raww` with a target token that is not a canonical
  configured session id
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

#### Scenario: Reject cross-bundle raww request in MVP

- **WHEN** caller invokes `raww` with bundle selector not matching associated
  bundle
- **THEN** relay returns `validation_cross_bundle_unsupported`

### Requirement: Relay raww target class gate

Relay raww recipients in MVP SHALL be configured coder transport sessions only
(`tmux` or `acp`).

Targets resolved to unsupported classes (including UI stream endpoints) SHALL
be rejected with `validation_invalid_params` and deterministic details
indicating unsupported target class.

#### Scenario: Reject ui target class for raww

- **WHEN** resolved raww target is a UI target class
- **THEN** relay returns `validation_invalid_params`
- **AND** error details indicate unsupported target class for raww

### Requirement: Relay raww authorization mapping

Relay SHALL evaluate raww authorization using policy control `raww`.

MVP policy scope contract:
- allowed values: `none`, `self`, `all:home`
- invalid values (including `all:all` and unknown values) SHALL fail
  configuration validation with `validation_invalid_policy_scope`

When raww is denied by policy, relay SHALL return
`authorization_forbidden` with canonical minimum details:
- `capability` = `raww.write`
- `requester_session`
- `bundle_name`
- `reason`

#### Scenario: Deny raww under self scope for non-self target

- **WHEN** requester policy sets `raww = "self"`
- **AND** requester invokes raww to another session in the same bundle
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include `capability = "raww.write"`

### Requirement: Relay raww transport behavior

Relay raww transport execution SHALL map as follows:
- tmux target: inject literal `text` into target pane; if `no_enter=false`,
  inject Enter after text
- acp target: submit `text` using existing shared ACP worker/client path via
  `session/prompt`

Relay SHALL treat raww `text` as opaque input and SHALL NOT evaluate shell
expansion or command substitution.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** raww target transport is `acp`
- **THEN** relay dispatches via existing shared ACP worker/client
  `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false`
- **AND** appends Enter after injected text

### Requirement: Relay raww response contract

Relay raww immediate success responses SHALL be acceptance-oriented only and
SHALL NOT guarantee terminal completion.

Required success fields:
- `status` (value `accepted`)
- `target_session`
- `transport`

Optional success fields:
- `request_id`
- `message_id`
- `details`

For ACP accepted success, relay SHALL include
`details.delivery_phase = "accepted_in_progress"`.
For tmux accepted success, relay MAY include
`details.delivery_phase = "accepted_dispatched"`.

Failure responses SHALL use canonical relay error payload shape (`code`,
`message`, optional `details`).

#### Scenario: Return deterministic accepted payload for acp raww

- **WHEN** raww request to acp target is accepted at dispatch boundary
- **THEN** relay returns success with `status = "accepted"`
- **AND** includes required fields `target_session` and `transport`
- **AND** includes `details.delivery_phase = "accepted_in_progress"`

### Requirement: Relay raww input bounds

Relay raww SHALL accept UTF-8 multiline text in MVP and SHALL reject payloads
larger than 32 KiB (UTF-8 bytes) with `validation_invalid_params`.

#### Scenario: Reject oversized raww text payload

- **WHEN** raww `text` exceeds 32 KiB UTF-8 bytes
- **THEN** relay rejects with `validation_invalid_params`
