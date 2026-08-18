## MODIFIED Requirements

### Requirement: Relay Host Startup Summary Contract

`agentmux host relay` SHALL expose a canonical machine startup summary payload.

The summary SHALL include:

- `schema_version`
- `host_mode` (`autostart`|`process_only`)
- `bundles` array with per-bundle entries:
  - `bundle_name`
  - `outcome` (`hosted`, `degraded`, `skipped`, `failed`)
  - `reason_code` (nullable)
  - `reason` (nullable human text)
  - `details` (nullable structured error details, preserved from the
    underlying relay error when one caused the outcome, or carrying
    `failed_sessions` when startup recorded per-session failures)
- `hosted_bundle_count`
- `degraded_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `hosted_any` (boolean)

`outcome=degraded` SHALL be reported for a bundle in which at least one
configured session reached ready state and at least one session startup attempt
failed. A partially started bundle SHALL NOT be reported as `hosted`. It remains
a hosted outcome: `hosted_any` SHALL be true when
`hosted_bundle_count + degraded_bundle_count > 0`, and a degraded bundle SHALL
NOT be counted in `failed_bundle_count`.

When a bundle's startup recorded per-session failures, `reason` SHALL name each
failed session and its cause, and `details.failed_sessions` SHALL carry the
structured per-session records. This SHALL hold for a `degraded` outcome as well
as a `failed` one, so the per-session causes reach the operator from the startup
summary itself rather than only from a subsequent `list`.

When a bundle is skipped due to runtime lock contention, `reason_code` SHALL be
`lock_held`.

CLI text output SHALL be a rendering layer over the same summary payload, and
SHALL render the failed session ids and causes for a `degraded` or `failed`
bundle.

In `host_mode=autostart`, process exit status SHALL reflect relay process
startup result and SHALL NOT fail solely because `hosted_bundle_count == 0`, nor
solely because a bundle is `degraded`.

When startup fails for one or more bundles and the host exits, each failed
bundle SHALL leave a per-bundle reason on stderr and in inscriptions before
the process exits.

#### Scenario: Emit startup summary in autostart mode

- **WHEN** relay host starts with no selector
- **THEN** summary payload sets `host_mode=autostart`

#### Scenario: Emit startup summary in process-only mode

- **WHEN** relay host starts with `--no-autostart`
- **THEN** startup outcomes are represented in the canonical machine payload
- **AND** `host_mode` is `process_only`

#### Scenario: Emit per-bundle failure reasons before fatal startup exit

- **WHEN** every autostart bundle fails to start
- **THEN** the host exits nonzero
- **AND** stderr carries a per-bundle failure reason with the structured
  error details
- **AND** inscriptions record a per-bundle startup failure event with the
  same structured details

#### Scenario: Report a partially started bundle as degraded

- **WHEN** an autostart bundle has at least one session reach ready state and
  at least one session startup attempt fail
- **THEN** the summary entry uses `outcome=degraded`
- **AND** `reason` names each failed session and its cause
- **AND** `details.failed_sessions` carries the structured per-session records
- **AND** the bundle is counted in `degraded_bundle_count`
- **AND** `hosted_any` is true
- **AND** the host does not exit nonzero on account of the degraded bundle

#### Scenario: Render failed session causes in startup text output

- **WHEN** the startup summary contains a `degraded` or `failed` bundle with
  recorded per-session failures
- **THEN** CLI text output names each failed session and its cause

### Requirement: Bundle Lifecycle Transition Summary Contract

`agentmux up` and `agentmux down` SHALL return canonical machine payloads.

The payload SHALL include:

- `schema_version`
- `action` (`up`|`down`)
- `bundles` array with per-bundle entries:
  - `bundle_name`
  - `outcome` (`hosted`|`unhosted`|`degraded`|`skipped`|`failed`)
  - `reason_code` (nullable)
  - `reason` (nullable)
  - `details` (nullable structured detail; carries `failed_sessions` when the
    transition recorded per-session startup failures)
- `changed_bundle_count`
- `degraded_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `changed_any` (boolean)

A configured session that fails to start SHALL NOT fail the `up` transition for
the bundle. Such a bundle SHALL report `outcome=degraded` when at least one
configured session is ready afterward, and `outcome=failed` when none is, per
the `bundle-lifecycle` capability's `Relay Bundle Lifecycle Result Contract`.
`changed_any` SHALL be true when
`changed_bundle_count + degraded_bundle_count > 0`.

`up/down` SHALL be idempotent:

- already hosted bundle in `up` returns `outcome=skipped` with
  `reason_code=already_hosted`
- already unhosted bundle in `down` returns `outcome=skipped` with
  `reason_code=already_unhosted`

CLI text output SHALL be a rendering layer over the same payload, and SHALL
render the failed session ids and causes for a `degraded` or `failed` bundle.

#### Scenario: Report idempotent already-hosted result for up

- **WHEN** operator runs `agentmux up relay`
- **AND** bundle `relay` is already hosted
- **THEN** result includes `outcome=skipped`
- **AND** `reason_code=already_hosted`

#### Scenario: Report idempotent already-unhosted result for down

- **WHEN** operator runs `agentmux down relay`
- **AND** bundle `relay` is already unhosted
- **THEN** result includes `outcome=skipped`
- **AND** `reason_code=already_unhosted`

#### Scenario: Bring a bundle up when one of its sessions fails

- **WHEN** operator runs `agentmux up relay`
- **AND** one configured session fails to start while another becomes ready
- **THEN** the command does not fail the transition
- **AND** the result entry uses `outcome=degraded`
- **AND** `details.failed_sessions` carries the failed session id and cause
- **AND** CLI text output names that session and its cause
