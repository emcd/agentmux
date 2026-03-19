## MODIFIED Requirements

### Requirement: Bundle Group Membership Field

Per-bundle TOML configuration SHALL support optional top-level bundle group
membership field:

- `groups` (`string[]`)

This field applies to bundle lifecycle command grouping (`up/down`) and SHALL
NOT change session routing identity semantics.

Group naming rules:

- reserved/system group names are uppercase
- custom group names are lowercase
- `ALL` is reserved and implicit

#### Scenario: Accept bundle file with custom groups

- **WHEN** bundle file includes `groups = ["dev", "login"]`
- **THEN** the system loads the bundle configuration successfully

#### Scenario: Accept bundle file without groups

- **WHEN** bundle file omits `groups`
- **THEN** the system loads the bundle configuration successfully

#### Scenario: Reject explicit ALL group in bundle groups

- **WHEN** bundle file includes `ALL` in `groups`
- **THEN** the system rejects configuration with
  `validation_reserved_group_name`

#### Scenario: Reject invalid uppercase custom group

- **WHEN** bundle file includes uppercase custom group name not reserved by
  system
- **THEN** the system rejects configuration with
  `validation_invalid_group_name`

## ADDED Requirements

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
