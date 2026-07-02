## MODIFIED Requirements

### Requirement: Bundle Autostart Eligibility Field

Per-bundle TOML configuration SHALL support optional top-level:

- `autostart` (boolean)
- `test-isolated` (boolean)

If `autostart` is omitted, it SHALL default to `false`. If `test-isolated`
is omitted, it SHALL default to `false`.

`autostart` SHALL only affect no-selector `agentmux host relay` autostart mode.

`test-isolated` SHALL indicate that the bundle is intended for use as a
test-harness target. A `test-isolated=true` bundle MUST be excluded
from the no-selector `agentmux host relay` autostart selection set,
regardless of its `autostart` value. The bundle MAY be hosted only via
the explicit `agentmux test bundle up <name>` command, which overrides
the autostart gate.

The TOML key MUST be `test-isolated` (kebab-case, per project TOML
practice); the corresponding Rust struct field is `test_isolated: bool`
with `#[serde(rename = "test-isolated")]` for the wire-form mapping.

`test-isolated` SHALL NOT change bundle routing identity semantics; it
only gates the auto-hosting path.

#### Scenario: Treat omitted autostart as false

- **WHEN** bundle file omits `autostart`
- **THEN** runtime resolves `autostart=false` for that bundle

#### Scenario: Resolve explicit autostart true

- **WHEN** bundle file sets `autostart = true`
- **THEN** runtime marks bundle as eligible for host autostart mode

#### Scenario: Treat omitted test-isolated as false

- **WHEN** bundle file omits `test-isolated`
- **THEN** runtime resolves `test-isolated=false` for that bundle

#### Scenario: Accept bundle file with test-isolated true

- **WHEN** bundle file sets `test-isolated = true`
- **AND** no `autostart` field is set
- **THEN** configuration loads successfully
- **AND** the `BundleConfiguration::test_isolated` field is `true`
- **AND** the `serde` rename from `test-isolated` (TOML) to
  `test_isolated` (Rust) is applied

#### Scenario: Test-isolated bundle is not autostart-eligible

- **WHEN** bundle file sets `test-isolated = true` (with or without
  `autostart = true`)
- **THEN** the no-selector `agentmux host relay` autostart selection
  set EXCLUDES this bundle
- **AND** the relay inscription logs the skip with the reason
  `bundle_test_isolated`

### Requirement: Host Relay No-Selector Autostart Resolution

When operator runs `agentmux host relay` with no selector mode, runtime SHALL:

1. start relay process,
2. select bundles with `autostart=true AND test-isolated!=true` (i.e.
   the autostart set, minus any test-isolated bundles),
3. attempt hosting selected bundles using existing per-bundle host semantics.

When operator runs `agentmux host relay --no-autostart`, runtime SHALL start
relay process and SHALL skip bundle hosting selection.

A test-isolated bundle MAY be hosted only via the explicit
`agentmux test bundle up <name>` command, which overrides the
autostart gate and hosts the bundle regardless of its `autostart` or
`test-isolated` values.

No-selector mode success SHALL be based on relay process startup success and
SHALL NOT fail solely because zero bundles were selected/hosted.

#### Scenario: Start relay and host eligible bundles in no-selector mode

- **WHEN** operator runs `agentmux host relay`
- **THEN** runtime starts relay process
- **AND** selects bundles where `autostart=true AND test-isolated!=true`
- **AND** attempts hosting those bundles

#### Scenario: Test-isolated bundle skipped at no-selector autostart

- **WHEN** operator runs `agentmux host relay`
- **AND** a bundle has `autostart = true AND test-isolated = true`
- **THEN** the bundle is NOT selected for hosting
- **AND** the relay inscription logs the skip with the reason
  `bundle_test_isolated`

#### Scenario: Test-isolated bundle hosted by test command override

- **WHEN** operator runs `agentmux test bundle up agentmux-test`
- **AND** `agentmux-test.toml` has `test-isolated = true`
- **THEN** the bundle IS hosted (overriding the autostart gate)
- **AND** the harness principal `qa-harness@agentmux-test` becomes
  reachable for scripted flows

#### Scenario: Start relay without bundle hosting in no-autostart mode

- **WHEN** operator runs `agentmux host relay --no-autostart`
- **THEN** runtime starts relay process
- **AND** does not perform bundle hosting selection
