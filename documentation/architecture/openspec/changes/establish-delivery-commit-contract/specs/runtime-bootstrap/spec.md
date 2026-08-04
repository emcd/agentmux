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

  | Key | Default | Range | Governs |
  |---|---|---|---|
  | `residency-ms` | `900_000` | `30_000..=3_600_000` | how long a `Pending` entry may wait before resolving `expired` |
  | `scheduling-quantum-bytes` | `262_144` | `65_536..=16_777_216` | a target's credit per rotation visit, in canonical payload bytes |
  | `submission-timeout-ms` | `30_000` | `1_000..=300_000` | how long an authorized batch's execution may run before the relay resolves it through the guard and initiates the fence |
  | `fence-join-timeout-ms` | `5_000` | `100..=60_000` | the budget for each of the generation fence's two cessation observations, so total acknowledgment is bounded by twice this value |
  | `queued-envelopes-max` | `10_000` | `1..=1_000_000` | relay-global admission quota, envelope count |
  | `queued-bytes-max` | `268_435_456` | `1_048_576..=4_294_967_296` | relay-global admission quota, canonical payload bytes |
  | `queued-envelopes-per-target-max` | `1_000` | `1..=1_000_000` | per-target admission quota, envelope count |
  | `queued-bytes-per-target-max` | `33_554_432` | `1_048_576..=4_294_967_296` | per-target admission quota, canonical payload bytes |

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

`residency-ms` and `submission-timeout-ms` bound different things and SHALL NOT
be conflated. Residency bounds how long the relay is willing to *wait to start*,
and expires a message that was never authorized. The submission timeout bounds
how long the relay lets its own already-started execution run. Their appropriate
durations differ by orders of magnitude — a pending wait legitimately spans an
agent's whole turn, while an authorized submission is a short write — which is
why one setting cannot serve both.

**Zero is not a permitted value for any `[delivery]` key, and no value denotes
"unlimited."** Every range above excludes zero, and a zero SHALL be rejected with
the same structured range error as any other out-of-range value. A zero quota
would reject every message and a zero fence-join bound would reap a child before
any executor could be joined, so overloading zero as "no limit" would make the
two most dangerous misconfigurations indistinguishable from the safest intent.

**Per-target quota SHALL NOT exceed relay-global quota in either dimension.**
`queued-envelopes-per-target-max` greater than `queued-envelopes-max`, or
`queued-bytes-per-target-max` greater than `queued-bytes-max`, SHALL fail
validation at load with a structured error naming both keys and both values. A
per-target limit above the global one is unreachable and therefore always a
configuration mistake.

`scheduling-quantum-bytes` SHALL be greater than or equal to the
**canonical-payload-byte component** of every registered transport's maximum
handover dimensions. A value below any registered byte component SHALL fail
validation at load with a structured error naming the key, the configured value,
and the transport whose byte component exceeds it. The envelope-count component
is not compared against the quantum, which is denominated in bytes.

Because transports register after configuration load, this constraint SHALL also
be revalidated when a transport registers or changes its declared maxima; see the
`delivery-quiescence` capability's `Async Queue Lifecycle and Ordering`
requirement for the refusal behavior.

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

#### Scenario: Bound authorized execution

- **WHEN** `relay.toml` sets `[delivery].submission-timeout-ms` within the
  permitted range
- **THEN** an authorized batch whose execution exceeds it is resolved through the
  guard's evidence order and its generation fence is initiated
- **AND** the setting is documented as an execution watchdog over the relay's own
  code, not as a judgement about target health

#### Scenario: Reject an out-of-range residency

- **WHEN** `[delivery].residency-ms` is below `30_000` or above `3_600_000`
- **THEN** relay startup fails with a structured error naming the key and the
  permitted range
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject a quantum below a registered byte maximum

- **WHEN** `[delivery].scheduling-quantum-bytes` is less than the
  canonical-payload-byte component of any registered transport's maximum handover
  dimensions
- **THEN** relay startup fails with a structured validation error naming the key,
  the configured value, and the transport whose byte component exceeds it

#### Scenario: Reject zero for a delivery setting

- **WHEN** any `[delivery]` key is set to `0`
- **THEN** relay startup fails with the structured range error for that key
- **AND** the zero is not interpreted as "unlimited"

#### Scenario: Reject per-target quota above the global quota

- **WHEN** `[delivery].queued-envelopes-per-target-max` exceeds
  `queued-envelopes-max`, or `queued-bytes-per-target-max` exceeds
  `queued-bytes-max`
- **THEN** relay startup fails with a structured validation error naming both
  keys and both values

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
