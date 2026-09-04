## MODIFIED Requirements

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
  - `list = home`
  - `look = home`
  - `send = home`

### Requirement: Authorization Control Vocabulary

Relay SHALL evaluate authorization using canonical controls and scope values:

- `list`: `none` | `self` | `home` | `all`
- `look`: `none` | `self` | `home` | `all`
- `send`: `none` | `self` | `home` | `all`

The policies file is authoritative: every control accepts the full scope
ladder at parse time, and the consuming authorization checks give each value
its effect via scope rank order.

A control SHALL NOT appear in this vocabulary before an authorization check
consumes it. Naming a control here obliges `policies.toml` to carry it, and a
key that every deployment must supply while nothing reads it is a cost with no
corresponding guarantee.

#### Scenario: Evaluate look request using configured look scope

- **WHEN** relay evaluates a `look` request
- **THEN** it uses the session policy control `look`
- **AND** applies one of the canonical scope values

## REMOVED Requirements

### Requirement: Authorization Hooks for Do and Find

**Reason**: The hooks reserve authorization for two verbs the system does not
provide. `find` is parsed into a required `policies.toml` key and then
discarded; `do` is parsed into a map and discarded. A key every deployment must
supply, backed by nothing that reads it, is a cost with no corresponding
guarantee. The requirement's own two scenarios describe relay denying by the
`do` control map, which a discarded map cannot do, so they have never been
satisfiable.

**Migration**: Remove `find` from `[policies.controls]` in every
`policies.toml`, including the shipped template. Because `RawPolicyControls`
carries `deny_unknown_fields`, the key must be removed in the same release that
removes the field: leaving it behind turns a startup that works into one that
refuses. `do` needs no migration — the key has a serde default and no shipped
policy defines a `[policies.controls.do]` block. When either verb is
implemented, its control returns through a proposal that adds the check and the
key together.

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
