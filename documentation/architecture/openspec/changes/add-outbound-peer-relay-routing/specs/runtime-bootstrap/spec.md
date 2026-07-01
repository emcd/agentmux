## ADDED Requirements

### Requirement: Relay Outbound Self Identity

Relay configuration SHALL support a top-level `relay-id` string in `relay.toml`
naming this relay's own bare relay id. When one or more `[[peers]]` entries are
configured, `relay-id` SHALL be present and non-empty; the relay presents
`<relay-id>@RELAY` as its own principal in the outbound Hello it sends to each
peer. That identity SHALL be one the target peer has registered (via
`new peer <relay-id>@RELAY`), so `relay-id` is the authenticating identity, not a
peer-local alias.

`relay-id` SHALL be a **bare relay id**: non-empty after trimming surrounding
whitespace, carrying no namespace suffix (`@`), no cross-relay delimiter (`!`),
and no path separators — consistent with the bare local-part grammar used for
session and peer ids (the relay composes the `@RELAY` suffix itself, so an
already-qualified value such as `foo@RELAY` is invalid rather than becoming
`foo@RELAY@RELAY`). A `relay.toml` that configures one or more `[[peers]]`
entries without a `relay-id`, or with a `relay-id` that is empty/whitespace or not
a bare relay id, SHALL fail startup and pre-flight configuration validation with a
structured validation error. `relay-id` is optional and unused when no `[[peers]]`
entry is configured.

#### Scenario: Relay id required when peers are configured

- **WHEN** `relay.toml` contains a `[[peers]]` entry but omits `relay-id` or
  supplies an empty `relay-id`
- **THEN** relay startup fails with a structured validation error naming
  `relay-id`
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Relay id optional without peers

- **WHEN** `relay.toml` configures no `[[peers]]` entries and omits `relay-id`
- **THEN** configuration validation accepts the file

#### Scenario: Reject qualified or malformed relay-id

- **WHEN** `relay.toml` sets `relay-id` to a value that is not a bare relay id —
  e.g. one carrying an `@` suffix (`foo@RELAY`), a `!` delimiter, a path
  separator, or only whitespace
- **THEN** relay startup fails with a structured validation error naming
  `relay-id`
- **AND** `agentmux check configuration` reports the same invalid artifact

## RENAMED Requirements

- FROM: `### Requirement: Peer Placeholder Configuration`
- TO: `### Requirement: Outbound Peer Relay Configuration`

## MODIFIED Requirements

### Requirement: Relay Configuration File

The runtime SHALL support a relay-level configuration artifact at
`<config-root>/relay.toml`. The file itself is the relay configuration table;
relay-wide keys SHALL NOT be nested under an additional `[relay]` table. The
file SHALL use kebab-case TOML keys and MAY contain:

- `watch-bundles` (boolean, default `true`)
- `require-session-credentials` (boolean, default `false`)
- `relay-id` (string; this relay's own `<relay-id>@RELAY` outbound identity —
  required when any `[[peers]]` entry is configured, otherwise optional and
  unused; see Relay Outbound Self Identity)
- `[choices].pending-max`
- top-level `[[peers]]` entries with required `id` and `address` string fields

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

#### Scenario: Accept relay-id key

- **WHEN** `relay.toml` contains a non-empty top-level `relay-id` string
- **THEN** configuration validation accepts the key as a known relay-level field

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
`require-session-credentials`. `[choices].pending-max`, `[[peers]]`, and
`relay-id` SHALL resolve from `relay.toml` or documented defaults only; this
proposal does not define CLI or environment overrides for those settings.

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

#### Scenario: No override for choices, peers, or relay-id

- **WHEN** `[choices].pending-max` is absent from `relay.toml`
- **AND** no `[[peers]]` entries exist in `relay.toml`
- **THEN** relay startup uses the documented choices default
- **AND** has no configured outbound peers
- **AND** no CLI or environment override resolves `relay-id`

### Requirement: Outbound Peer Relay Configuration

Relay configuration SHALL support top-level `[[peers]]` entries that define
outbound peer relay routing. `[[peers]]` is purely an outbound routing table; it
carries no inbound authorization. Each peer entry SHALL carry:

- `id`: a required non-empty string equal to the peer relay's canonical
  `<id>@RELAY` principal. The bare `<id>` portion (without the `@RELAY` suffix)
  serves as the peer's `<relay_id>` in cross-relay target addressing and as its
  `<peer_alias>` for the credential file path.
- `address`: a required outbound endpoint. In this slice `address` SHALL be an
  **absolute filesystem path** to a Unix domain socket (same-host peers), the
  transport the relay presently serves. A non-absolute value, or a `host:port`
  TCP-style endpoint, SHALL be rejected at startup and pre-flight validation with
  a structured error — the fail-fast counterpart of the remote/TCP non-goal,
  rather than deferring the failure to an unreachable-socket delivery outcome. A
  `host:port` TCP endpoint is the documented future shape once the relay gains a
  TCP listener and is not yet a supported target.

Inbound authorization for a peer relay — what an inbound request carried by that
peer may reach on this relay — is NOT configured here. It is the `scope` recorded
on the peer relay principal's store record when its credential is registered via
`new peer <id>@RELAY`, and is read by the ingress filter (see the
`relay-routing-layer` capability). A relay that only receives from a peer
therefore needs no `[[peers]]` entry for it — only a registered credential.

Unknown peer entry fields SHALL fail startup and pre-flight configuration
validation with structured validation errors. Peer entries SHALL NOT contain raw
PSK material; raw peer relay PSKs SHALL remain owner-only state artifacts at
`<state-root>/peers/<peer-alias>.psk` (mode 0600), while the principal store
records credential hashes.

The relay SHALL NOT open an outbound peer connection at startup solely because a
peer entry exists; connections are established lazily on first cross-relay
delivery to that peer (see the `cross-relay-routing` capability). A peer whose
endpoint is unreachable at startup SHALL NOT block or fail relay startup.

#### Scenario: Validate outbound peer entry

- **WHEN** `relay.toml` contains a `[[peers]]` entry with a non-empty `id`
  parsing to a `<id>@RELAY` principal and an absolute `address` Unix socket path,
  and a non-empty top-level `relay-id`
- **THEN** configuration validation accepts the entry
- **AND** relay startup does not attempt an outbound peer connection

#### Scenario: Reject non-absolute or TCP-style peer address

- **WHEN** a `[[peers]]` entry's `address` is not an absolute path — e.g. a
  `host:port` TCP endpoint or a relative path
- **THEN** relay startup fails with a structured validation error naming the
  `peers.address` field
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject peer entry missing id

- **WHEN** a `[[peers]]` entry omits `id` or supplies an empty `id`
- **THEN** relay startup fails with a structured validation error naming the
  offending `peers.id` field
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject malformed peer entry

- **WHEN** a `[[peers]]` entry omits `address` or carries an unknown field
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact
