## ADDED Requirements

All requirements below are moved verbatim from `openspec/specs/session-relay/spec.md` as part of the `partition-session-relay-spec` refactor (see `todos/general/31`).

### Requirement: Bundle Reconciliation

The system SHALL provide a reconciliation operation that ensures all known
bundle sessions are online under the same host user.

#### Scenario: Start missing session during reconciliation

- **WHEN** reconciliation runs and a configured session is absent
- **THEN** the system creates that tmux session
- **AND** starts the configured coder command in the configured working
  directory

#### Scenario: Keep existing session during reconciliation

- **WHEN** reconciliation runs and a configured session already exists
- **THEN** the system leaves that session running

#### Scenario: Reconciliation does not depend on start-server only

- **WHEN** reconciliation needs to bring a missing session online
- **THEN** the system creates the session directly
- **AND** does not treat `tmux start-server` alone as sufficient readiness

### Requirement: Reconciliation Lifecycle Policy

The system SHALL implement startup and cleanup behavior that minimizes session
creation races and avoids leaking idle tmux servers.

#### Scenario: Bootstrap then parallel session creation

- **WHEN** multiple configured sessions are missing during reconciliation
- **THEN** the system creates one deterministic bootstrap session first
- **AND** creates remaining missing sessions in parallel after bootstrap

#### Scenario: Retry transient creation races

- **WHEN** session creation fails with a transient tmux readiness error
- **THEN** the system retries with bounded attempts
- **AND** applies short jitter between retries

#### Scenario: Track agentmux-owned sessions

- **WHEN** the system creates a session during reconciliation
- **THEN** the system marks that session as agentmux-owned using tmux metadata

#### Scenario: Cleanup dedicated socket server only when fully idle

- **WHEN** reconciliation or pruning finds zero agentmux-owned sessions on a
  dedicated configured socket and zero total sessions remain on that socket
- **THEN** the system shuts down that socket's tmux server
- **AND** does not require `exit-empty` to be turned off for startup

#### Scenario: Preserve socket server while non-owned sessions exist

- **WHEN** reconciliation or pruning finds zero agentmux-owned sessions on a
  dedicated configured socket but non-owned sessions remain
- **THEN** the system does not shut down that socket's tmux server

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

### Requirement: Relay Bundle Lifecycle Result Contract

Relay bundle lifecycle responses for `up/down` SHALL include:

- `schema_version`
- `action` (`up`|`down`)
- `bundles` array entries with:
  - `bundle_name`
  - `outcome` (`hosted`|`unhosted`|`skipped`|`failed`)
  - `reason_code` (nullable)
  - `reason` (nullable)
- aggregate fields:
  - `changed_bundle_count`
  - `skipped_bundle_count`
  - `failed_bundle_count`
  - `changed_any`

For `up`, lock contention MAY produce:

- `outcome=skipped`
- `reason_code=lock_held`

#### Scenario: Emit canonical up lifecycle payload

- **WHEN** relay completes an `up` operation
- **THEN** response matches canonical lifecycle result contract

#### Scenario: Emit canonical down lifecycle payload

- **WHEN** relay completes a `down` operation
- **THEN** response matches canonical lifecycle result contract

### Requirement: Bundle Configuration Includes Autostart Eligibility

Per-bundle TOML configuration SHALL support optional top-level `autostart`
boolean with default `false`.

`autostart` SHALL indicate eligibility for no-selector relay host autostart mode
and SHALL NOT change bundle routing identity semantics.

#### Scenario: Accept bundle file with autostart true

- **WHEN** bundle file includes `autostart = true`
- **THEN** configuration loads successfully

#### Scenario: Accept bundle file without autostart field

- **WHEN** bundle file omits `autostart`
- **THEN** configuration loads successfully
- **AND** runtime treats bundle as not autostart-eligible

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

Per-transport readiness predicates:

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

Persisted history contract:

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

### Requirement: Dynamic Bundle File Watching

The relay host SHALL watch the bundles configuration directory for filesystem
changes when resolved relay configuration leaves `watch-bundles` absent or
`true`. The watcher SHALL use a debounced reconcile-on-change model: on any
debounced notification the relay re-scans the full directory and reconciles the
loaded bundle set against the on-disk set. Debounce window SHALL be short enough
for interactive use (~200ms) and long enough to avoid acting on partial writes.

On a new bundle file: relay SHALL load and start the bundle runtime, equivalent
to `bundle up`. Validation errors on the new file are recorded as startup
failures; the relay continues serving other bundles unchanged.

On a disappeared bundle file: relay SHALL emit a typed error frame
(`runtime_bundle_unloaded`) to every active session in that bundle, close all
connections for that bundle, and unload the bundle from the runtime catalog.
Principal store entries for the affected sessions SHALL be retained (the
principal store is relay-level and not pruned on bundle unload).

On a modified bundle file: relay SHALL treat the change as a disappear followed
by a new file: emit `runtime_bundle_reloaded`, disconnect all active sessions,
then reload the bundle with the new configuration.

When resolved relay configuration sets `watch-bundles = false`, the relay SHALL
NOT start the bundle file watcher and SHALL ignore bundle file add/remove/modify
events until relay restart.

#### Scenario: New bundle file detected at runtime

- **WHEN** a new bundle TOML file appears in the bundles directory
- **AND** the relay is running with watching enabled by `relay.toml` or defaults
- **THEN** relay loads and starts the new bundle without restart
- **AND** subsequent connections to that bundle succeed

#### Scenario: Bundle file removed at runtime with active sessions

- **WHEN** a bundle TOML file is removed from the bundles directory
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_unloaded` to each active session before
  disconnect
- **AND** closes all connections for that bundle
- **AND** unloads the bundle from the runtime catalog
- **AND** subsequent connection attempts for that bundle return
  `validation_unknown_bundle`

#### Scenario: Bundle file modified at runtime with active sessions

- **WHEN** a bundle TOML file is modified
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_reloaded` to each active session before
  disconnect
- **AND** closes all connections
- **AND** reloads the bundle with the new configuration

#### Scenario: Watch disabled by relay configuration

- **WHEN** relay configuration resolves `watch-bundles = false`
- **AND** a bundle file is added, removed, or modified at runtime
- **THEN** relay does NOT reconcile the bundle set
- **AND** changes take effect only after relay restart
