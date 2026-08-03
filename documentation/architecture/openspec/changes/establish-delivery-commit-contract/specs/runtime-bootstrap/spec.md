## MODIFIED Requirements

### Requirement: Relay Configuration File

The runtime SHALL support a relay-level configuration artifact at
`<config-root>/relay.toml`. The file itself is the relay configuration table;
relay-wide keys SHALL NOT be nested under an additional `[relay]` table. The
file SHALL use kebab-case TOML keys and MAY contain:

- `watch-bundles` (boolean, default `true`)
- `require-session-credentials` (boolean, default `false`)
- `[choices].pending-max`
- `[delivery]` table governing the relay's delivery patience and scheduling:
  - `residency-ms` (default `900_000`, range `30_000..=3_600_000`) — how long a
    `Pending` entry may wait before resolving `expired`
  - `scheduling-quantum-bytes` — the deficit round-robin quantum per rotation
    visit, in canonical payload bytes
  - `fence-join-timeout-ms` — the bound on a generation fence's executor join
    before its escalation applies
  - `queued-envelopes-max` and `queued-bytes-max` — relay-global admission quota
  - `queued-envelopes-per-target-max` and `queued-bytes-per-target-max` —
    per-target admission quota
- top-level `[[peers]]` entries with required `alias`, `address`, and
  `connect-as` string fields (see Outbound Peer Relay Configuration and Relay
  Cross-Relay Presented Identity)

The `[delivery]` keys live here rather than in `coders.toml` because they
describe the relay's own patience and scheduling rather than any coder's
behavior. Per-target residency overrides are deliberately excluded from this
schema.

`residency-ms` SHALL default to a value exceeding the longest plausible agent
turn, since a target mid-turn is legitimately not ready and its message must
wait. The lower bound SHALL keep an operator from configuring a value beneath a
single turn, and the upper bound SHALL keep the setting from re-creating an
effectively unbounded wait.

`scheduling-quantum-bytes` SHALL be greater than or equal to every registered
transport's maximum handover dimension. A value below any registered maximum
SHALL fail validation at load with a structured error naming the key, the
configured value, and the transport whose maximum exceeds it.

Missing `relay.toml` SHALL use the documented defaults. Malformed `relay.toml`,
unknown fields, wrong field types, and invalid peer entries SHALL fail startup
and pre-flight configuration validation with structured validation errors.

#### Scenario: Defaults when relay.toml is absent

- **WHEN** the configuration root has no `relay.toml`
- **THEN** relay startup uses `watch-bundles = true`
- **AND** uses `require-session-credentials = false`
- **AND** uses the documented `[delivery]` defaults
- **AND** has no configured outbound peers

#### Scenario: Load explicit relay controls

- **WHEN** `relay.toml` contains `watch-bundles = false`
- **AND** `require-session-credentials = true`
- **THEN** relay startup uses those relay-level settings

#### Scenario: Load explicit delivery patience settings

- **WHEN** `relay.toml` contains a `[delivery]` table setting `residency-ms`
  within the permitted range
- **THEN** relay startup uses that value as the bound on how long any transport's
  `Pending` entries may wait
- **AND** the value applies uniformly across every transport

#### Scenario: Reject an out-of-range residency

- **WHEN** `[delivery].residency-ms` is below `30_000` or above `3_600_000`
- **THEN** relay startup fails with a structured error naming the key and the
  permitted range
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject a quantum below a registered handover maximum

- **WHEN** `[delivery].scheduling-quantum-bytes` is less than any registered
  transport's maximum handover dimension
- **THEN** relay startup fails with a structured validation error naming the key,
  the configured value, and the transport whose maximum exceeds it

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
