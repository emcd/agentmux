## ADDED Requirements

### Requirement: Relay Configuration File

The runtime SHALL support a relay-level configuration artifact at
`<config-root>/relay.toml`. The file itself is the relay configuration table;
relay-wide keys SHALL NOT be nested under an additional `[relay]` table. The
file SHALL use kebab-case TOML keys and MAY contain:

- `watch-bundles` (boolean, default `true`)
- `require-session-credentials` (boolean, default `false`)
- `[choices].pending-max`
- top-level `[[peers]]` entries with required `address` string fields

Missing `relay.toml` SHALL use the documented defaults. Malformed `relay.toml`,
unknown fields, wrong field types, and invalid peer entries SHALL fail startup
and pre-flight configuration validation with structured validation errors.

#### Scenario: Defaults when relay.toml is absent

- **WHEN** the configuration root has no `relay.toml`
- **THEN** relay startup uses `watch-bundles = true`
- **AND** uses `require-session-credentials = false`
- **AND** has no configured outbound peers

#### Scenario: Load explicit relay controls

- **WHEN** `relay.toml` contains `watch-bundles = false`
- **AND** `require-session-credentials = true`
- **THEN** relay startup uses those relay-level settings

#### Scenario: Reject nested relay table

- **WHEN** `relay.toml` contains a `[relay]` table
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject malformed relay TOML syntax

- **WHEN** `relay.toml` is not syntactically valid TOML
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject unknown relay configuration field

- **WHEN** `relay.toml` contains an unknown top-level field
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject wrong relay configuration field type

- **WHEN** `relay.toml` contains `watch-bundles = 'false'`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

### Requirement: Relay Configuration Precedence

Relay runtime settings SHALL resolve with this precedence, highest to lowest:
CLI override, environment override, `relay.toml`, documented defaults. CLI
overrides SHALL include `agentmux host relay --no-watch` for
`watch-bundles = false` and `agentmux host relay --require-credentials` for
`require-session-credentials = true`. Environment overrides SHALL include
`AGENTMUX_RELAY_WATCH_BUNDLES` and
`AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS`, parsed as canonical boolean
strings: exactly `true` or `false`. Invalid environment override values SHALL
fail startup with structured validation errors.

This precedence ladder applies to `watch-bundles` and
`require-session-credentials`. `[choices].pending-max` and `[[peers]]` SHALL
resolve from `relay.toml` or documented defaults only; this proposal does not
define CLI or environment overrides for those settings.

#### Scenario: CLI override wins over relay.toml

- **WHEN** `relay.toml` contains `watch-bundles = true`
- **AND** the operator runs `agentmux host relay --no-watch`
- **THEN** relay startup resolves `watch-bundles = false`

#### Scenario: Environment override wins over relay.toml

- **WHEN** `relay.toml` contains `require-session-credentials = false`
- **AND** `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS=true` is set
- **THEN** relay startup resolves `require-session-credentials = true`

#### Scenario: Accept canonical boolean environment override values

- **WHEN** `AGENTMUX_RELAY_WATCH_BUNDLES=false` is set
- **AND** `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS=true` is set
- **THEN** relay startup accepts both environment override values

#### Scenario: relay.toml wins over defaults

- **WHEN** `relay.toml` contains `watch-bundles = false`
- **AND** no CLI or environment override is supplied for watch behavior
- **THEN** relay startup resolves `watch-bundles = false`

#### Scenario: Reject invalid environment override

- **WHEN** `AGENTMUX_RELAY_WATCH_BUNDLES=maybe` is set
- **THEN** relay startup fails with a structured validation error

#### Scenario: No override for choices or peers

- **WHEN** `[choices].pending-max` is absent from `relay.toml`
- **AND** no `[[peers]]` entries exist in `relay.toml`
- **THEN** relay startup uses the documented choices default
- **AND** has no configured outbound peers

### Requirement: Peer Placeholder Configuration

Relay configuration SHALL support top-level `[[peers]]` entries as schema
placeholders for future outbound peer relay routing. Each peer entry SHALL carry
a required non-empty `address` string. Unknown peer entry fields SHALL fail
startup and pre-flight configuration validation with structured validation
errors. Peer entries SHALL NOT contain raw PSK material; raw peer relay PSKs
SHALL remain owner-only state artifacts at
`<state-root>/peers/<peer-alias>.psk`, while the principal store records
credential hashes.

The relay SHALL NOT open outbound peer connections, advertise peer targets, or
change routing behavior solely because a peer entry exists.

#### Scenario: Validate peer placeholder entry

- **WHEN** `relay.toml` contains `[[peers]]` with a non-empty `address`
- **THEN** configuration validation accepts the entry
- **AND** relay startup does not attempt an outbound peer connection

#### Scenario: Reject malformed peer placeholder entry

- **WHEN** `relay.toml` contains a `[[peers]]` entry without `address`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject empty peer address

- **WHEN** `relay.toml` contains a `[[peers]]` entry with `address = ''`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject unknown peer field

- **WHEN** `relay.toml` contains a `[[peers]]` entry with an unknown field
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Peer entry alone does not change routing

- **WHEN** `relay.toml` contains one or more valid `[[peers]]` entries
- **THEN** relay startup does not advertise those peers as routable targets
- **AND** relay startup does not open outbound connections to those peers

#### Scenario: Peer PSK omitted from relay configuration

- **WHEN** `relay.toml` contains a valid `[[peers]]` entry
- **THEN** the entry contains no raw PSK value
- **AND** peer relay PSK material remains under `<state-root>/peers/`
