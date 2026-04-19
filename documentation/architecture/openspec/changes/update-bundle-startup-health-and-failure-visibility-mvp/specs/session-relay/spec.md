## ADDED Requirements

### Requirement: Bundle Startup Evaluation Boundary

Relay bundle startup SHALL evaluate outcomes in two deterministic phases:

1. bundle preflight phase,
2. per-session startup pass phase.

When preflight succeeds, relay SHALL attempt startup for all configured
sessions in that bundle during one startup pass.
Startup outcome SHALL be computed after that startup pass completes.

When preflight fails, relay SHALL:

- mark bundle state as `down`,
- set `state_reason_code=runtime_startup_failed`,
- skip the per-session startup pass.

Per-transport readiness predicates in MVP:

- tmux session is ready when configured session exists and relay resolves an
  active pane target.
- ACP session is ready when shared per-target ACP worker reaches ready state and
  lifecycle selection succeeds (`session/load` or `session/new` per existing
  contract).

#### Scenario: Attempt all configured sessions after successful preflight

- **WHEN** preflight succeeds for a bundle startup request
- **THEN** relay attempts startup for all configured sessions in that bundle
- **AND** relay evaluates startup outcome only after the pass completes

#### Scenario: Fail preflight before per-session startup pass

- **WHEN** bundle preflight fails
- **THEN** relay marks bundle `state=down`
- **AND** sets `state_reason_code=runtime_startup_failed`
- **AND** does not run the per-session startup pass

### Requirement: Bundle Startup Health Model

Relay list payloads SHALL preserve bundle `state` as `up|down`.
For `state=up`, relay SHALL include required additive field
`startup_health` with value `healthy|degraded`.

Startup health semantics:

- `state=up`, `startup_health=healthy` when all configured sessions are ready.
- `state=up`, `startup_health=degraded` when at least one configured session is
  ready and at least one startup attempt failed.
- `state=down` when zero configured sessions are ready.

For empty bundles (`members=[]`), relay SHALL return:

- `state=down`
- `state_reason_code=runtime_no_configured_sessions`

#### Scenario: Return degraded startup health with partial session success

- **WHEN** at least one configured session becomes ready
- **AND** at least one configured session startup attempt fails
- **THEN** relay reports `state=up`
- **AND** includes `startup_health=degraded`

#### Scenario: Return down state for zero ready sessions

- **WHEN** zero configured sessions are ready after startup evaluation
- **THEN** relay reports `state=down`

#### Scenario: Return empty-bundle down reason

- **WHEN** bundle configuration contains zero sessions
- **THEN** relay reports `state=down`
- **AND** sets `state_reason_code=runtime_no_configured_sessions`

### Requirement: Startup Failure Visibility Contract

Relay SHALL provide machine-readable startup failure visibility via:

1. live per-session startup failure event/inscription:
   `relay.session_start_failed`,
2. persisted bounded per-bundle startup failure history.

Persisted history contract in MVP:

- fixed bound `max_startup_failures=256`,
- oldest-first eviction when bound is exceeded,
- response ordering oldest -> newest,
- monotonic per-bundle `sequence` field per failure record,
- history persists across relay restarts,
- history clears when bundle runtime state is explicitly reset/removed.

Each startup-failure record SHALL include:

- `bundle_name`
- `session_id`
- `transport` (`tmux`|`acp`)
- `code`
- `reason`
- `timestamp`
- `sequence`
- optional `details`

Relay list payloads SHALL include:

- `startup_failure_count` (required integer),
- `recent_startup_failures` (required bounded array; may be empty).

#### Scenario: Emit canonical startup-failure event

- **WHEN** one session startup attempt fails during startup pass
- **THEN** relay emits `relay.session_start_failed`
- **AND** event payload includes canonical startup-failure fields

#### Scenario: Expose bounded startup-failure history in list payload

- **WHEN** startup-failure history exists for a bundle
- **THEN** relay list payload includes `startup_failure_count`
- **AND** includes `recent_startup_failures` ordered oldest -> newest

#### Scenario: Evict oldest startup-failure history record at bound

- **WHEN** a new startup-failure record is persisted and bundle history already
  contains 256 records
- **THEN** relay evicts the oldest record first

### Requirement: Bundle Down Reason Precedence

When relay reports `state=down`, `state_reason_code` precedence SHALL be:

1. `runtime_no_configured_sessions` (empty bundle),
2. `runtime_startup_failed` (preflight failure or all configured sessions
   failed startup pass).

Relay SHALL preserve process-level host startup summary semantics for
`runtime_listener_bind_failed`; this code is not part of bundle list-state
reason precedence.

#### Scenario: Prefer no-configured-sessions reason over startup-failed reason

- **WHEN** bundle has zero configured sessions
- **THEN** relay reports `state_reason_code=runtime_no_configured_sessions`

#### Scenario: Use startup-failed reason when startup pass yields zero ready sessions

- **WHEN** bundle preflight succeeds
- **AND** startup pass completes with zero ready sessions
- **THEN** relay reports `state_reason_code=runtime_startup_failed`

## MODIFIED Requirements

### Requirement: Relay Bundle Lifecycle Operations

Relay SHALL support explicit bundle lifecycle transition operations:

- `up` (host selected bundle runtimes)
- `down` (unhost selected bundle runtimes)

These operations SHALL control bundle hosting state and SHALL NOT terminate the
relay process itself.

`up/down` SHALL be idempotent:

- `up` on an already hosted bundle returns `outcome=skipped` with
  `reason_code=already_hosted`
- `down` on an already unhosted bundle returns `outcome=skipped` with
  `reason_code=already_unhosted`

`up/down` result payloads SHALL preserve selector-resolved bundle order.

Bundle startup outcomes SHALL be scoped to bundle lifecycle evaluation and SHALL
NOT relock process-level no-selector `agentmux host relay` startup success
semantics.

#### Scenario: Keep relay process alive after down transition

- **WHEN** relay processes `down` for one or more bundles
- **THEN** relay updates bundle hosting state
- **AND** relay process remains running

#### Scenario: Report idempotent up transition

- **WHEN** relay processes `up` for a bundle already hosted by current runtime
- **THEN** result entry uses `outcome=skipped`
- **AND** sets `reason_code=already_hosted`

#### Scenario: Report idempotent down transition

- **WHEN** relay processes `down` for a bundle not currently hosted
- **THEN** result entry uses `outcome=skipped`
- **AND** sets `reason_code=already_unhosted`
