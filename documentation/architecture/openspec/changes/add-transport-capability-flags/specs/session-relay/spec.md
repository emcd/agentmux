## ADDED Requirements

### Requirement: Transport Capability Contract

Every target reachable via look or raww SHALL have transport capabilities
derivable from its `SessionType` at check time:

- `can_be_looked: bool` — the session can be targeted by `look` (its transport
  supports snapshot capture)
- `can_be_written: bool` — the session can be targeted by `raww` (its transport
  supports raw input injection)
- `can_stream_output: bool` — the session's transport natively produces live
  output chunks (ACP and PTY stream output natively; Tmux requires periodic
  polling)

Capabilities SHALL be derived from the target's `SessionType` (from
`BundleMember` configuration for bundle targets, or from `TuiSession::session_type`
in `users.toml` for relay-wide targets):

| Transport | `can_be_looked` | `can_be_written` | `can_stream_output` |
|-----------|----------------|----------------|-------------------|
| `Tmux` | true | true | false |
| `Acp` | true | true | true |
| `Pty` | true | true | true |
| `Ui` | false | false | false |
| `Pubsub` | false | false | false |

`can_stream_output` is advertised on registration; streaming look semantics
that consume it are deferred to a follow-on proposal.

When a look or raww operation resolves a target whose transport type has the
relevant capability false, relay SHALL return `validation_unsupported_operation`.
This check precedes authorization policy checks and applies to both bundle
targets (checked from `BundleMember` configuration) and relay-wide targets
(checked from `users.toml` session type).

#### Scenario: Reject look against session with can_be_looked false

- **WHEN** a `look` request resolves to a target whose transport type carries
  `can_be_looked = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Reject raww against session with can_be_written false

- **WHEN** a `raww` request resolves to a target whose transport type carries
  `can_be_written = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Permit look against session with can_be_looked true

- **WHEN** a `look` request resolves to a target whose transport type carries
  `can_be_looked = true`
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: Permit raww against session with can_be_written true

- **WHEN** a `raww` request resolves to a target whose transport type carries
  `can_be_written = true`
- **THEN** relay proceeds to authorization policy evaluation

## MODIFIED Requirements

### Requirement: Relay Raww Target Routing

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

Relay-wide (`@GLOBAL`) targets are no longer rejected at the routing stage;
rejection occurs at the capability check using `validation_unsupported_operation`
when the resolved session carries `can_be_written = false`. This separates namespace
routing from operation-capability concerns.

Validation precedence SHALL evaluate target qualification (at the resolution
stage), then target existence, then capability, then authorization policy checks.

Raww and Look are complementary single-target operations and SHALL share one
config-free resolution stage; their reserved namespace target rejection is
uniform.

After this change, the routing stage for look and raww SHALL resolve `@GLOBAL`
targets as relay-wide rather than rejecting them at the routing stage; the
handler then derives the resolved target's session type and applies the
capability check. The `RelayWideTargets` enum and `resolve_target`'s
relay-wide-targets parameter are removed in this change — dead code once the
single `Rejected` call site is gone.

#### Scenario: Reject unqualified raww target

- **WHEN** caller invokes `raww` with a target without `@<namespace>` suffix
- **THEN** relay returns `validation_unqualified_target`

#### Scenario: Reject reserved namespace raww target

- **WHEN** caller invokes `raww` with an `@EXTERNAL` or `@RELAY` target
- **THEN** relay returns `validation_unsupported_namespace`

#### Scenario: Reject relay-wide raww target via capability check

- **WHEN** caller invokes `raww` with an `@GLOBAL` (relay-wide) target
- **THEN** relay returns `validation_unsupported_operation`
- **AND** the rejection is uniform with the look capability check for the
  same target

#### Scenario: Cross-bundle raww denied by scope

- **WHEN** caller invokes `raww` with a target in a different bundle
- **AND** requester's `raww` scope is `home` or narrower
- **THEN** relay returns `authorization_forbidden`

## REMOVED Requirements

### Requirement: Relay raww target class gate

Superseded by the Transport Capability Contract. The relay-wide (`@GLOBAL`)
single-target rejection at the routing stage — the `RelayWideTargets::Rejected`
short-circuit that currently emits `validation_unsupported_namespace` in
`routing.rs` — is retired; `validation_unsupported_operation` from the capability
check in the handler body is the canonical rejection for transport-unsupported
targets. The `RelayWideTargets` enum and `resolve_target`'s relay-wide-targets
parameter are removed in this change — dead code once the single `Rejected`
call site is gone.
