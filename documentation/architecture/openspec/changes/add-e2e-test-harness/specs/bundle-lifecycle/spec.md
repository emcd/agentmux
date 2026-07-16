## MODIFIED Requirements

### Requirement: Bundle Configuration Includes Autostart Eligibility

Per-bundle TOML configuration SHALL support optional top-level `autostart`
boolean with default `false`.

`autostart` SHALL indicate eligibility for no-selector relay host autostart mode
and SHALL NOT change bundle routing identity semantics.

Per-bundle TOML configuration SHALL also support optional top-level
`test-isolated` boolean with default `false`. The TOML key MUST be
`test-isolated` (kebab-case, per project TOML practice); the
corresponding Rust struct field is `test_isolated: bool` with
`#[serde(rename = "test-isolated")]` for the wire-form mapping.
`test-isolated` SHALL indicate that the bundle is intended for use
as a test-harness target. `test-isolated` SHALL NOT change bundle
routing identity semantics; the autostart-selection impact of
`test-isolated=true` (exclusion from no-selector autostart) is
governed by `specs/runtime-bootstrap/spec.md`.

#### Scenario: Accept bundle file with autostart true

- **WHEN** bundle file includes `autostart = true`
- **THEN** configuration loads successfully

#### Scenario: Accept bundle file without autostart field

- **WHEN** bundle file omits `autostart`
- **THEN** configuration loads successfully
- **AND** runtime treats bundle as not autostart-eligible

#### Scenario: Accept bundle file with test-isolated true

- **WHEN** bundle file includes `test-isolated = true`
- **THEN** configuration loads successfully
- **AND** the `BundleConfiguration::test_isolated` field is `true`
- **AND** the `serde` rename from `test-isolated` (TOML) to
  `test_isolated` (Rust) is applied

#### Scenario: Accept bundle file without test-isolated field

- **WHEN** bundle file omits `test-isolated`
- **THEN** configuration loads successfully
- **AND** runtime treats the bundle as not test-isolated
