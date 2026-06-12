# session-relay Spec Delta

## MODIFIED Requirements

### Requirement: Authorization Control Vocabulary

Relay SHALL evaluate authorization using canonical controls and scope values:

- `find`: `none` | `self` | `home` | `all`
- `list`: `none` | `self` | `home` | `all`
- `look`: `none` | `self` | `home` | `all`
- `send`: `none` | `self` | `home` | `all`
- `do`: map `action_id -> (none | self | home | all)`

The policies file is authoritative: every control accepts the full scope
ladder at parse time, and the consuming authorization checks give each value
its effect via scope rank order.

For current self-target-only `do` MVP behavior:

- `none` and `self` are operative
- `home` and `all` are reserved/non-operative until non-self `do`
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
- **AND** action scope is `home` or `all`
- **THEN** relay treats scope as reserved/non-operative for current MVP
- **AND** non-self `do` execution remains unsupported by runtime contract

### Requirement: Permission Decision Capability Contract

Relay SHALL evaluate ACP permission-request decision authority using policy
capability `grant`.

- allowed values: `none`, `self`, `home`, `all`
- default when omitted: `none`
- unknown values SHALL fail validation with `validation_invalid_policy_scope`

#### Scenario: Reject unknown grant scope value

- **WHEN** policy configuration sets `grant` to a value outside the canonical
  scope ladder
- **THEN** relay rejects configuration with `validation_invalid_policy_scope`

#### Scenario: Default omitted grant to none

- **WHEN** policy omits `grant`
- **THEN** relay treats `grant` as `none`
