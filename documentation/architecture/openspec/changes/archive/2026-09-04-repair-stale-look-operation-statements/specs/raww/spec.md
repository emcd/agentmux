## MODIFIED Requirements

### Requirement: Relay raww target resolution and bundle boundary

Raww targets SHALL be resolved using the shared single-target routing stage.

Validation behavior:

- bare/unqualified target (no `@<namespace>` suffix) →
  `validation_unqualified_target`
- reserved namespace (`@EXTERNAL`/`@RELAY`) target →
  `validation_unsupported_namespace`
- unknown/non-canonical target → `validation_unknown_target`
- resolved target with `can_be_written = false` →
  `validation_unsupported_operation` (see Transport Capability Contract)
- cross-bundle raww with insufficient scope → `authorization_forbidden`

Relay-wide (`@GLOBAL`) targets SHALL NOT be rejected at the routing stage.
Rejection occurs after it, against the target's unified registry entry: an
unregistered principal SHALL be rejected with `validation_unknown_target`, and a
registered one whose transport carries `can_be_written = false` with
`validation_unsupported_operation`. This separates namespace routing from
operation-capability concerns.

Validation precedence SHALL evaluate target qualification (at the resolution
stage), then target existence, then capability, then authorization policy checks.

Raww and Look are complementary single-target operations and SHALL share one
config-free resolution stage; their reserved namespace target rejection is
uniform. That stage SHALL resolve `@GLOBAL` targets as relay-wide for both
operations rather than rejecting them, and each operation's handler SHALL then
derive the resolved target's session type and apply its own capability check.

#### Scenario: Reject unqualified raww target

- **WHEN** caller invokes `raww` with a target without `@<namespace>` suffix
- **THEN** relay returns `validation_unqualified_target`

#### Scenario: Reject reserved namespace raww target

- **WHEN** caller invokes `raww` with an `@EXTERNAL` or `@RELAY` target
- **THEN** relay returns `validation_unsupported_namespace`

#### Scenario: Reject registered relay-wide raww target via capability check

- **WHEN** caller invokes `raww` with a registered `@GLOBAL` (relay-wide) target
  whose transport carries `can_be_written = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** the rejection is uniform with the look capability check for the
  same target

#### Scenario: Reject unregistered relay-wide raww target

- **WHEN** caller invokes `raww` with an `@GLOBAL` (relay-wide) target that is
  not a registered principal
- **THEN** relay returns `validation_unknown_target`
