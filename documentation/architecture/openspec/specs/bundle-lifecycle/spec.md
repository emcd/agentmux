# bundle-lifecycle Specification

## Purpose

Reconciliation, bundle up/down, startup health, file watching.
## Requirements
### Requirement: Bundle Reconciliation

The system SHALL provide a reconciliation operation that ensures all known
bundle sessions are online under the same host user.

A configured session that fails to come online SHALL NOT prevent reconciliation
from attempting the remaining configured sessions. Reconciliation SHALL record
the per-session cause and continue, and SHALL report the recorded failures to
its caller. This matches the startup path, which already tolerates and records a
per-session failure; a bundle is a set of sessions, and one failing is not a
reason to withhold the rest.

Reconciliation SHALL bring up every configured session through the same
per-session startup step the startup path uses, and SHALL evaluate readiness from
its result rather than from what reconciliation created. A session whose
transport has no tmux session to create SHALL be started by that step rather than
judged by observation alone; reporting such a session as failed because
reconciliation never attempted to start it is a defect. A session that was
started but is not ready SHALL be recorded as a failure rather than reported as a
success.

Errors that are not attributable to a single session — the bundle being absent
from the runtime catalog, principal registration failing, or the session-state
query itself failing — SHALL continue to fail the whole operation.

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

#### Scenario: Continue reconciling after a session fails to start

- **WHEN** reconciliation runs and one configured session fails to be created
- **AND** other configured sessions are still absent
- **THEN** the system attempts those remaining sessions
- **AND** records the failed session's id and cause
- **AND** reports the recorded failures to its caller

#### Scenario: Record a created session that does not become ready

- **WHEN** reconciliation creates a configured session
- **AND** that session does not satisfy the readiness condition
- **THEN** the system records it as a failed session with its cause
- **AND** does not count it toward the bundle's ready sessions

#### Scenario: Start a configured session that has no tmux session to create

- **WHEN** reconciliation runs for a bundle whose configured sessions include a
  transport that creates no tmux session
- **THEN** the system starts that session through the same per-session startup
  step the startup path uses
- **AND** does not record it as failed on the grounds that it was not already
  running

#### Scenario: Evaluate readiness for an already-running session

- **WHEN** reconciliation runs and a configured session already exists
- **THEN** the system evaluates that session's readiness
- **AND** counts it toward the bundle's ready sessions only when it is ready

#### Scenario: Fail the whole operation for a non-session-scoped error

- **WHEN** reconciliation cannot query tmux session state, or principal
  registration fails
- **THEN** the operation fails
- **AND** no partial result is reported in its place

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
  - `outcome` (`hosted`|`unhosted`|`degraded`|`skipped`|`failed`)
  - `reason_code` (nullable)
  - `reason` (nullable)
  - `details` (nullable structured detail; carries `failed_sessions` when the
    transition recorded per-session startup failures)
- aggregate fields:
  - `changed_bundle_count`
  - `degraded_bundle_count`
  - `skipped_bundle_count`
  - `failed_bundle_count`
  - `changed_any`

`outcome=degraded` SHALL be reported for an `up` transition in which at least
one configured session is ready afterward and at least one configured session
failed to start. Readiness SHALL be the same condition the `Bundle Startup
Health Model` requirement uses, so `degraded` names the same state on this
contract that `startup_health` names on a list payload. One session SHALL NOT be
ready on one of those surfaces and not ready on the other: a single readiness
condition per transport SHALL serve both, since a word that resolves differently
depending on which surface is asked names nothing. It is a hosted outcome:
`changed_any` SHALL be true when
`changed_bundle_count + degraded_bundle_count > 0`, and a degraded bundle SHALL
NOT be counted in `failed_bundle_count`.

`outcome=failed` SHALL be reported for an `up` transition in which no configured
session is ready afterward, whether or not sessions were created.

When per-session failures were recorded, `reason` SHALL name each failed session
and its cause, and `details.failed_sessions` SHALL carry the structured
per-session records. The `degraded` spelling is the same one the
`Bundle Startup Health Model` requirement uses for a bundle with at least one
ready session and at least one failed startup attempt.

For `up`, lock contention MAY produce:

- `outcome=skipped`
- `reason_code=lock_held`

#### Scenario: Emit canonical up lifecycle payload

- **WHEN** relay completes an `up` operation
- **THEN** response matches canonical lifecycle result contract

#### Scenario: Emit canonical down lifecycle payload

- **WHEN** relay completes a `down` operation
- **THEN** response matches canonical lifecycle result contract

#### Scenario: Report a partially started bundle as degraded

- **WHEN** relay completes an `up` operation for a bundle in which one
  configured session failed to start and another is ready
- **THEN** the result entry uses `outcome=degraded`
- **AND** `reason` names the failed session and its cause
- **AND** `details.failed_sessions` carries the structured per-session record
- **AND** `changed_any` is true
- **AND** the bundle is not counted in `failed_bundle_count`

#### Scenario: Report an entirely failed bundle as failed

- **WHEN** relay completes an `up` operation for a bundle in which every
  configured session failed to start
- **THEN** the result entry uses `outcome=failed`
- **AND** `details.failed_sessions` carries every per-session record

#### Scenario: Treat a created-but-not-ready session as failed, not present

- **WHEN** relay completes an `up` operation for a bundle in which one
  configured session was created but did not become ready
- **AND** another configured session failed to start
- **THEN** the created-but-not-ready session is recorded as a failure
- **AND** no configured session is ready
- **AND** the result entry uses `outcome=failed` rather than `degraded`

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
- ACP session is ready when its shared per-target worker can serve and lifecycle
  selection succeeds (`session/load` or `session/new` per existing contract). A
  worker busy with an in-flight turn can serve and SHALL be reported ready: an
  agent's turn can last minutes, and treating it as unready would report a
  healthy bundle as `degraded` — or a single-member bundle as `down` — for that
  whole duration.

Each per-transport predicate SHALL be evaluated identically wherever readiness is
asserted, so a bring-up outcome and a list payload never disagree about whether
one session is ready.

#### Scenario: Attempt all configured sessions after successful preflight

- **WHEN** preflight succeeds for a bundle startup request
- **THEN** relay attempts startup for all configured sessions in that bundle
- **AND** relay evaluates startup outcome only after the pass completes

#### Scenario: Fail preflight before per-session startup pass

- **WHEN** bundle preflight fails
- **THEN** relay marks bundle `state=down`
- **AND** sets `state_reason_code=runtime_startup_failed`
- **AND** does not run the per-session startup pass

#### Scenario: Report a busy ACP session as ready

- **WHEN** a configured ACP session's worker is serving an in-flight turn
- **THEN** the session is reported ready on the bring-up outcome and the list
  payload alike
- **AND** the bundle's state is not lowered on account of that session

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

The relay host SHALL watch the effective bundles configuration for filesystem
changes when resolved relay configuration leaves `watch-bundles` absent or
`true`. The watcher SHALL observe the bundles directory of **every** configuration
layer, and SHALL reconcile against the **effective union** produced by the shared
effective-file lookup, in which an entry in an earlier layer shadows an entry of
the same bundle identifier in a later layer.

The watcher SHALL use a debounced reconcile-on-change model: on any debounced
notification the relay re-scans every layer, recomputes the effective union, and
reconciles the loaded bundle set against it. Debounce window SHALL be short
enough for interactive use (~200ms) and long enough to avoid acting on partial
writes.

Notification SHALL NOT be the only trigger. Filesystem notification is
best-effort on every supported platform: a backend may drop events under load,
and a debouncer may cancel events against each other before delivering them, so
a change can produce no notification at all. The relay SHALL therefore also
reconcile on a recurring interval, short enough that a change whose notification
was lost is picked up without operator intervention. A reconciliation the
interval triggers SHALL be indistinguishable in effect from one a notification
triggers, both re-scanning every layer and reconciling against the effective
union.

Reconciliation SHALL be driven by change in the **effective** bundle set, not by
the physical file event:

- A new effective bundle: relay SHALL load and start the bundle runtime,
  equivalent to `bundle up`. Validation errors on the new file are recorded as
  startup failures; the relay continues serving other bundles unchanged.
- A file appearing in a layer that shadows a bundle currently supplied by a later
  layer SHALL be a **reload** of that bundle, not an unload followed by an
  unrelated load.
- A file being deleted while a file of the same identifier remains in a later
  layer SHALL be a **reload** onto the revealed definition, and SHALL NOT unload
  the bundle.
- An edit to a file that is currently shadowed by an entry in an earlier layer
  SHALL produce no effective change and SHALL NOT reload or disconnect sessions.
- A bundle disappearing from the effective union entirely: relay SHALL emit a
  typed error frame (`runtime_bundle_unloaded`) to every active session in that
  bundle, close all connections for that bundle, and unload the bundle from the
  runtime catalog. Principal store entries for the affected sessions SHALL be
  retained (the principal store is relay-level and not pruned on bundle unload).
- A modified effective definition: relay SHALL treat the change as a disappear
  followed by a new file: emit `runtime_bundle_reloaded`, disconnect all active
  sessions, then reload the bundle with the new configuration.

Reconciliation SHALL distinguish definitions by the physical file supplying them
as well as by content, so a file that is byte-identical to the one it shadows
still reloads. A bundle's relative path is the same in every layer — that is
what makes them the same identifier — so two layers may hold byte-identical
definitions differing only in which physical file supplies them. Content alone
therefore cannot tell a shadowing event from no event at all, and provenance
must participate in the comparison.

When resolved relay configuration sets `watch-bundles = false`, the relay SHALL
NOT start the bundle file watcher and SHALL ignore bundle file add/remove/modify
events in any layer until relay restart.

#### Scenario: New bundle file detected at runtime

- **WHEN** a new bundle TOML file appears in the bundles directory of a layer
- **AND** no earlier layer shadows that identifier
- **AND** the relay is running with watching enabled by `relay.toml` or defaults
- **THEN** relay loads and starts the new bundle without restart
- **AND** subsequent connections to that bundle succeed

#### Scenario: A file appearing in an earlier layer shadows a loaded bundle

- **WHEN** a bundle file appears in a layer earlier than the one currently
  supplying a loaded bundle of the same identifier
- **THEN** relay reloads that bundle from the earlier layer's definition
- **AND** emits `runtime_bundle_reloaded` rather than `runtime_bundle_unloaded`

#### Scenario: Deletion reveals a later layer's definition

- **WHEN** a bundle file is deleted from the layer currently supplying it
- **AND** a file of the same identifier exists in a later layer
- **THEN** relay reloads that bundle from the revealed definition
- **AND** the bundle is not unloaded

#### Scenario: Edit to a shadowed file is inert

- **WHEN** a bundle file is modified
- **AND** an entry of the same identifier in an earlier layer shadows it
- **THEN** the effective definition is unchanged
- **AND** no reload or session disconnection occurs

#### Scenario: Byte-identical shadowing still reloads

- **WHEN** a bundle file appears in an earlier layer whose content is identical
  to the definition it shadows
- **THEN** relay reloads that bundle from the earlier layer
- **AND** deleting it again reloads from the revealed definition

#### Scenario: A change whose notification never arrives is still reconciled

- **WHEN** the effective bundle set changes
- **AND** no filesystem notification for that change is delivered to the relay
- **THEN** relay reconciles the change on the next recurring pass
- **AND** the resulting load, unload, or reload is the same as it would have been
  had the notification arrived

#### Scenario: Bundle removed from the effective union with active sessions

- **WHEN** a bundle's last remaining definition is removed from every layer
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_unloaded` to each active session before
  closing connections
- **AND** unloads the bundle from the runtime catalog

#### Scenario: Effective definition modified at runtime with active sessions

- **WHEN** the effective definition of a loaded bundle changes, whether by
  editing the file that currently supplies it in any layer
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_reloaded` to each active session before
  disconnect
- **AND** closes all connections
- **AND** reloads the bundle with the new configuration

#### Scenario: Watching disabled ignores every layer

- **WHEN** resolved relay configuration sets `watch-bundles = false`
- **THEN** no watcher is started for the bundles directory of any layer

