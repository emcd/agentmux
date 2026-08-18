## MODIFIED Requirements

### Requirement: Bundle Startup Health Model

Relay list payloads SHALL preserve bundle `state` as `up|down`.
For `state=up`, relay SHALL include required additive field
`startup_health` with value `healthy|degraded`.

A configured session SHALL be counted ready when its transport reports it
serving at the time the payload is produced. Readiness SHALL be evaluated per
payload rather than carried forward from a startup pass, so a session that has
since recovered counts ready and a session that has since stopped serving does
not.

Startup health semantics:

- `state=up`, `startup_health=healthy` when all configured sessions are ready.
- `state=up`, `startup_health=degraded` when at least one configured session is
  ready and at least one configured session is not ready.
- `state=down` when zero configured sessions are ready.

Startup health SHALL be derived from readiness alone. Persisted startup-failure
history SHALL NOT be an input to `startup_health`: the history records why a
session failed to start, which is a different question from whether it is
serving now.

For empty bundles (`members=[]`), relay SHALL return:

- `state=down`
- `state_reason_code=runtime_no_configured_sessions`

#### Scenario: Return degraded startup health with partial session readiness

- **WHEN** at least one configured session is ready
- **AND** at least one configured session is not ready
- **THEN** relay reports `state=up`
- **AND** includes `startup_health=degraded`

#### Scenario: Return healthy startup health for a recovered session

- **WHEN** a configured session's startup attempt failed earlier
- **AND** every configured session is ready when the payload is produced
- **THEN** relay reports `startup_health=healthy`

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

The persisted history is a diagnostic record of startup attempts that failed and
have not since been superseded by that session serving. A record SHALL NOT
outlive the condition it describes.

Persisted history contract:

- fixed bound `max_startup_failures=256`,
- oldest-first eviction when bound is exceeded,
- response ordering oldest -> newest,
- monotonic per-bundle `sequence` field per failure record,
- history persists across relay restarts,
- history clears when bundle runtime state is explicitly reset/removed,
- every record for one session clears when that session is next observed serving
  successfully, whether observed by a successful startup or by a successful
  delivery to it,
- clearing SHALL apply on every such observation, not only the first for a
  session, so a session that fails and recovers more than once leaves no record
  of a superseded failure behind.

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

#### Scenario: Clear a session's failure records when it is next observed serving

- **WHEN** a session has one or more persisted startup-failure records
- **AND** that session is next observed serving successfully
- **THEN** relay clears every startup-failure record for that session
- **AND** leaves records for other sessions in the bundle untouched

#### Scenario: Clear a session's failure records on a later recovery

- **WHEN** a session's startup-failure records were cleared by an earlier
  observation of it serving
- **AND** a further startup attempt for that session fails and is recorded
- **AND** that session is again observed serving successfully
- **THEN** relay clears the later startup-failure record as well
