## MODIFIED Requirements

### Requirement: Bundle Reconciliation

The system SHALL provide a reconciliation operation that ensures all known
bundle sessions are online under the same host user.

A configured session that fails to come online SHALL NOT prevent reconciliation
from attempting the remaining configured sessions. Reconciliation SHALL record
the per-session cause and continue, and SHALL report the recorded failures to
its caller. This matches the startup path, which already tolerates and records a
per-session failure; a bundle is a set of sessions, and one failing is not a
reason to withhold the rest.

Reconciliation SHALL evaluate readiness for every configured session, not only
for sessions it created, and SHALL apply the same readiness condition the
startup path applies. A session that was created but is not ready SHALL be
recorded as a failure rather than reported as a success.

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
contract that `startup_health` names on a list payload. It is a hosted outcome:
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
