## ADDED Requirements

### Requirement: Relay Cross-Relay Presented Identity

The identity this relay presents to a peer SHALL be configured **per peer**, not
relay-wide: the *receiving* relay determines the identity it expects (via its own
`new peer`), and two peers MAY issue this relay different — or colliding —
identities, so no single relay-wide identity exists. Each `[[peers]]` entry SHALL
carry a `connect-as` string naming the bare relay id that peer issued this relay;
the relay composes `<connect-as>@RELAY` and presents it as its own principal in
the outbound Hello it sends to that peer. A relay that only receives from a peer
needs no `[[peers]]` entry and presents no identity to it.

`connect-as` SHALL be a **bare relay id**: non-empty after trimming surrounding
whitespace, carrying no namespace suffix (`@`), no cross-relay delimiter (`!`),
and no path separators — the relay composes the `@RELAY` suffix itself, so an
already-qualified value such as `east@RELAY` is invalid rather than becoming
`east@RELAY@RELAY`. A `[[peers]]` entry that omits `connect-as`, or supplies one
that is empty/whitespace or not a bare relay id, SHALL fail startup and pre-flight
configuration validation with a structured validation error.

#### Scenario: Reject qualified or malformed connect-as

- **WHEN** a `[[peers]]` entry sets `connect-as` to a value that is not a bare
  relay id — e.g. one carrying an `@` suffix (`east@RELAY`), a `!` delimiter, a
  path separator, or only whitespace
- **THEN** relay startup fails with a structured validation error naming the
  `peers.connect-as` field
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
- `[choices].pending-max`
- top-level `[[peers]]` entries with required `alias`, `address`, and
  `connect-as` string fields (see Outbound Peer Relay Configuration and Relay
  Cross-Relay Presented Identity)

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

### Requirement: Outbound Peer Relay Configuration

Relay configuration SHALL support top-level `[[peers]]` entries that define
outbound peer relay routing. `[[peers]]` is purely an outbound routing table; it
carries no inbound authorization. Each peer entry SHALL carry:

- `alias`: a required non-empty string — this relay's **local** name for the
  peer. It serves as the peer's `<alias>` in cross-relay bang-path addressing
  (`<session>@<bundle>!<alias>`) and as the `<alias>` in the credential file path.
  It is internal to this relay and never presented to the peer. Grammar: a bare
  relay id (non-empty; no `@`, `!`, or path separator).
- `address`: a required outbound endpoint. In this slice `address` SHALL be an
  **absolute filesystem path** to a Unix domain socket (same-host peers), the
  transport the relay presently serves. A non-absolute value, or a `host:port`
  TCP-style endpoint, SHALL be rejected at startup and pre-flight validation with
  a structured error — the fail-fast counterpart of the remote/TCP non-goal,
  rather than deferring the failure to an unreachable-socket delivery outcome. A
  `host:port` TCP endpoint is the documented future shape once the relay gains a
  TCP listener and is not yet a supported target.
- `connect-as`: a required non-empty bare relay id — the identity this relay
  presents to the peer (`<connect-as>@RELAY`), determined by the peer (see Relay
  Cross-Relay Presented Identity).

Inbound authorization for a peer relay — what an inbound request carried by that
peer may reach on this relay — is NOT configured here. It is the `scope` recorded
on the peer relay principal's store record when its credential is registered via
`new peer <id>@RELAY`, and is read by the ingress filter (see the
`relay-routing-layer` capability). A relay that only receives from a peer
therefore needs no `[[peers]]` entry for it — only a registered credential.

Unknown peer entry fields SHALL fail startup and pre-flight configuration
validation with structured validation errors. Peer entries SHALL NOT contain raw
PSK material; raw peer relay PSKs SHALL remain owner-only state artifacts at
`<state-root>/peers/<alias>.psk` (mode 0600), while the principal store records
credential hashes.

The relay SHALL NOT open an outbound peer connection at startup solely because a
peer entry exists; connections are established lazily on first cross-relay
delivery to that peer (see the `cross-relay-routing` capability). A peer whose
endpoint is unreachable at startup SHALL NOT block or fail relay startup.

#### Scenario: Validate outbound peer entry

- **WHEN** `relay.toml` contains a `[[peers]]` entry with a non-empty `alias`, an
  absolute `address` Unix socket path, and a non-empty bare-id `connect-as`
- **THEN** configuration validation accepts the entry
- **AND** relay startup does not attempt an outbound peer connection

#### Scenario: Reject non-absolute or TCP-style peer address

- **WHEN** a `[[peers]]` entry's `address` is not an absolute path — e.g. a
  `host:port` TCP endpoint or a relative path
- **THEN** relay startup fails with a structured validation error naming the
  `peers.address` field
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject peer entry missing alias or connect-as

- **WHEN** a `[[peers]]` entry omits (or leaves empty) `alias` or `connect-as`
- **THEN** relay startup fails with a structured validation error naming the
  offending field
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject malformed peer entry

- **WHEN** a `[[peers]]` entry omits `address` or carries an unknown field
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact
