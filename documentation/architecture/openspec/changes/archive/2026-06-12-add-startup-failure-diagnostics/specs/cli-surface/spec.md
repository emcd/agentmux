# cli-surface Spec Delta

## MODIFIED Requirements

### Requirement: Relay Host Startup Summary Contract

`agentmux host relay` SHALL expose a canonical machine startup summary payload.

The summary SHALL include:

- `schema_version`
- `host_mode` (`autostart`|`process_only`)
- `bundles` array with per-bundle entries:
  - `bundle_name`
  - `outcome` (`hosted`, `skipped`, `failed`)
  - `reason_code` (nullable)
  - `reason` (nullable human text)
  - `details` (nullable structured error details, preserved from the
    underlying relay error when one caused the outcome)
- `hosted_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `hosted_any` (boolean)

When a bundle is skipped due to runtime lock contention, `reason_code` SHALL be
`lock_held`.

CLI text output SHALL be a rendering layer over the same summary payload.

In `host_mode=autostart`, process exit status SHALL reflect relay process
startup result and SHALL NOT fail solely because `hosted_bundle_count == 0`.

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
