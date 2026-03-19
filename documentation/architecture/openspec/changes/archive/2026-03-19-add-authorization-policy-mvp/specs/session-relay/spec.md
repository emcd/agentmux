## ADDED Requirements
### Requirement: Policy Preset Source

Relay authorization policy presets SHALL be loaded from:

- `<config-root>/policies.toml`

`policies.toml` SHALL define presets using `[[policies]]` entries with:

- `id` (required)
- `description` (optional)
- `[controls]` (required)

`policies.toml` MAY define:

- `default` (`<policy-id>`)

Relay SHALL fail fast when this artifact is missing or invalid.

#### Scenario: Reject startup when policies file is missing

- **WHEN** relay starts and `<config-root>/policies.toml` is absent
- **THEN** relay fails startup with a validation/runtime error
- **AND** relay does not continue with implicit fallback policy

#### Scenario: Reject startup when policies file is invalid

- **WHEN** relay starts and `policies.toml` cannot be parsed or validated
- **THEN** relay fails startup with a validation/runtime error
- **AND** relay does not continue with partial policy state

#### Scenario: Use built-in conservative default when preset default is absent

- **WHEN** `policies.toml` omits top-level `default`
- **AND** a session omits explicit `policy`
- **THEN** relay applies built-in conservative default policy
- **AND** built-in controls are:
  - `find = self`
  - `list = all:home`
  - `look = self`
  - `send = all:home`
  - `do` defaults to `none` for unspecified actions

### Requirement: Session Policy Binding

Each session definition SHALL support optional binding to a policy preset id:

- `policy = "<policy-id>"`

If session `policy` is omitted, relay SHALL resolve policy by precedence:

1. top-level `default` preset in `policies.toml` when present
2. built-in conservative default policy

Relay SHALL reject bundle configuration when a session references an unknown
policy id.

#### Scenario: Reject unknown session policy reference

- **WHEN** a session declares `policy = "missing-policy"`
- **AND** `policies.toml` has no matching `[[policies]].id`
- **THEN** relay rejects configuration with a validation error

#### Scenario: Resolve omitted session policy from top-level default

- **WHEN** session omits explicit `policy`
- **AND** `policies.toml` defines top-level `default`
- **THEN** relay uses that default policy preset for the session

### Requirement: Authorization Control Vocabulary

Relay SHALL evaluate authorization using canonical controls and scope values:

- `find`: `self` | `all:home` | `all:all`
- `list`: `all:home` | `all:all`
- `look`: `self` | `all:home` | `all:all`
- `send`: `all:home` | `all:all`
- `do`: map `action_id -> (none | self | all:home | all:all)`

For current self-target-only `do` MVP behavior:

- `none` and `self` are operative
- `all:home` and `all:all` are reserved/non-operative until non-self `do`
  targeting is introduced

#### Scenario: Evaluate look request using configured look scope

- **WHEN** relay evaluates a `look` request
- **THEN** it uses the session policy control `look`
- **AND** applies one of the canonical scope values

#### Scenario: Treat missing do action entry as none

- **WHEN** relay evaluates `do` authorization
- **AND** requested action id is not present in `do` control map
- **THEN** relay treats authorization scope as `none`

#### Scenario: Treat do all-home/all-all scopes as reserved in current MVP

- **WHEN** relay evaluates `do` authorization
- **AND** action scope is `all:home` or `all:all`
- **THEN** relay treats scope as reserved/non-operative for current MVP
- **AND** non-self `do` execution remains unsupported by runtime contract

### Requirement: Centralized Authorization Decision Point

Relay SHALL be the centralized authorization decision point.
CLI and MCP SHALL remain validators/adapters and SHALL NOT implement
independent authorization decision logic.

#### Scenario: Return relay-authored denial across surfaces

- **WHEN** a request is denied by policy
- **THEN** relay returns canonical denial response
- **AND** CLI/MCP propagate that denial without re-evaluating authorization

### Requirement: Authorization Evaluation Order

Relay SHALL evaluate requests in this order:

1. request validation
2. requester identity resolution
3. bundle/target/action resolution
4. authorization policy evaluation
5. execution

Validation failures SHALL take precedence over authorization denials.

#### Scenario: Prefer validation failure over authorization denial

- **WHEN** a request includes an unknown target session
- **THEN** relay returns `validation_unknown_target`
- **AND** relay does not return `authorization_forbidden` for that request

### Requirement: Authorization Denial Schema

When relay denies a valid/resolved request by policy, relay SHALL return
`authorization_forbidden` with `details` including:

- required:
  - `capability`
  - `requester_session`
  - `bundle_name`
  - `reason`
- optional:
  - `target_session`
  - `targets`
  - `policy_rule_id`

#### Scenario: Return canonical denial details for single-target operation

- **WHEN** relay denies a same-bundle non-self look request by policy
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include required fields
- **AND** denial details include `target_session`

### Requirement: Relay List Authorization

Relay list responses SHALL require policy evaluation for capability `list.read`.
If requester identity is valid and list access is denied by policy, relay SHALL
return `authorization_forbidden` and SHALL NOT return an empty successful list.

#### Scenario: Deny list without returning empty success payload

- **WHEN** requester identity is valid
- **AND** policy denies list visibility for that requester
- **THEN** relay returns `authorization_forbidden`
- **AND** relay does not return `recipients=[]` as a success response

### Requirement: Relay Send Scope Control

Relay send authorization SHALL be driven by `send` control scope:

- `all:home` allows only same-bundle targets
- `all:all` allows cross-bundle targets when runtime support/trust path exists

#### Scenario: Reject cross-bundle send under home-only scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "all:home"`
- **THEN** relay returns `authorization_forbidden`

### Requirement: Authorization Hooks for Do and Find

Relay SHALL reserve authorization hooks for:

- `do` action-id scoped controls
- `find` scope controls

These hooks SHALL use the same evaluation order and denial schema as `list`,
`send`, and `look`.

#### Scenario: Deny do action run with canonical schema

- **WHEN** relay denies action execution by `do` control map
- **THEN** relay returns `authorization_forbidden`
- **AND** details include canonical required fields

#### Scenario: Deny do action run when do map sets none

- **WHEN** requested action id maps to `none` in `do` control map
- **THEN** relay returns `authorization_forbidden`

## MODIFIED Requirements
### Requirement: MVP Trust Boundary

The system SHALL operate in a same-host, same-user trust boundary for MVP.
Authorization principal identity SHALL be association/socket-driven requester
identity resolved by runtime context.
Caller-supplied sender-like payload fields SHALL NOT override principal
identity.

#### Scenario: Operate against current user's tmux server

- **WHEN** delivery or reconciliation executes
- **THEN** the system targets tmux resources owned by the current host user

#### Scenario: Ignore caller-supplied sender override for principal identity

- **WHEN** caller supplies sender-like payload field that conflicts with
  associated requester identity
- **THEN** relay authorizes against associated requester identity
- **AND** does not treat payload override as authoritative

### Requirement: Relay Look Operation

The system SHALL provide a relay-level read-only inspection operation:
`look`.

`look` request fields SHALL include:

- `requester_session` (required)
- `target_session` (required)
- `lines` (optional)
- `bundle_name` (optional/redundant when bundle is already bound by
  association/socket context)

MVP authorization posture for `look` SHALL be:

- default scope `self`
- broader scope controlled by policy (`all:home` or `all:all`)
- cross-bundle look currently unsupported by runtime contract

#### Scenario: Resolve bundle from associated runtime context

- **WHEN** look request omits `bundle_name`
- **THEN** relay resolves bundle from associated runtime context

#### Scenario: Accept redundant matching bundle name

- **WHEN** look request includes `bundle_name` matching associated runtime
  context
- **THEN** relay accepts request and proceeds with the look operation

#### Scenario: Reject mismatched bundle name in MVP

- **WHEN** look request includes `bundle_name` that does not match
  associated runtime context
- **THEN** relay rejects request with `validation_cross_bundle_unsupported`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** requester and target are different sessions in same bundle
- **AND** requester policy has `look = "self"`
- **THEN** relay returns `authorization_forbidden`
