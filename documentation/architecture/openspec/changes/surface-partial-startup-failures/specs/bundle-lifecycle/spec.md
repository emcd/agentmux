## MODIFIED Requirements

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
