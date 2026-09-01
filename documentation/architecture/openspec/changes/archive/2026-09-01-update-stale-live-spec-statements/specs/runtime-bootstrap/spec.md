## MODIFIED Requirements

### Requirement: Relay Configuration File

The runtime SHALL support a relay-level configuration artifact at
`<config-root>/relay.toml`. The file itself is the relay configuration table;
relay-wide keys SHALL NOT be nested under an additional `[relay]` table. The
file SHALL use kebab-case TOML keys and MAY contain:

- `watch-bundles` (boolean, default `true`)
- `require-session-credentials` (boolean, default `false`)
- `[choices].pending-max`
- `[delivery]` table governing the relay's delivery scheduling, admission, and
  queue observability:

  | Key | Default | Range | Governs |
  |---|---|---|---|
  | `submission-timeout-ms` | `5_000` | `500..=60_000` | how long an authorized batch's ingestion may run before the relay initiates the generation fence |
  | `fence-observation-timeout-ms` | `5_000` | `100..=60_000` | the budget for each of the generation fence's two cessation observations, so total acknowledgment is bounded by twice this value |
  | `unreachable-dwell-ms` | `30_000` | `1_000..=600_000` | how long a target may be **continuously** unreachable before its still-waiting members resolve |
  | `queued-envelopes-max` | `10_000` | `1..=1_000_000` | relay-global admission quota, envelope count |
  | `queued-bytes-max` | `268_435_456` | `1_048_576..=4_294_967_296` | relay-global admission quota, canonical payload bytes |
  | `queued-envelopes-per-target-max` | `1_000` | `1..=1_000_000` | per-target admission quota, envelope count |
  | `queued-bytes-per-target-max` | `33_554_432` | `1_048_576..=4_294_967_296` | per-target admission quota, canonical payload bytes |
  | `undelivered-warning-ms` | `1_800_000` | `60_000..=86_400_000` | how long a target's oldest `Pending` entry may age before the relay emits that target's first-crossing warning inscription |
  | `undelivered-report-interval-ms` | `300_000` | `30_000..=3_600_000` | cadence of the periodic undelivered-queue aggregate inscription |

- top-level `[[peers]]` entries with required `alias`, `address`, and
  `connect-as` string fields (see Outbound Peer Relay Configuration and Relay
  Cross-Relay Presented Identity)

The `[delivery]` keys live here rather than in `coders.toml` because they
describe the relay's own queue, scheduling, and reporting rather than any coder's
behavior.

**No `[delivery]` key bounds how long the relay waits for a *reachable* target to
become ready, and no configuration SHALL introduce one.** Such an entry waits
without a duration bound, subject to one deliberate exception — a fail-stopped
worker resolves every member it holds rather than stranding it behind a
generation that can never be replaced; see the `delivery-quiescence` capability's
`Async Queue Lifecycle and Ordering` requirement. `unreachable-dwell-ms` is not
an exception to this: it bounds how long a target may be continuously
*unreachable*, which qualifies a repeated observation rather than substituting
for an absent one.

`submission-timeout-ms` is the sole post-authorization bound, and it SHALL NOT be
read as a readiness bound. **It bounds ingestion, not readiness.** A batch is
authorized only once the relay has observed the target ready, and no transport
may wait on readiness afterwards, so the clock never covers a readiness wait.
What it covers is the transport consuming the bytes — in practice a single write
into a pty master, a child's stdin, or a subscriber channel.

Because readiness is advisory and can go stale between check and authorization,
**stale readiness is precisely how ingestion stalls**: the relay believed the
target was draining, began pushing bytes, and the target stopped. This is why the
default is small. Ingestion into a genuinely draining target completes in
microseconds; seconds of blocked ingestion mean the reader is not draining, not
that the write is large.

**Zero is not a permitted value for any `[delivery]` key, and no value denotes
"unlimited."** Every range above excludes zero, and a zero SHALL be rejected with
the same structured range error as any other out-of-range value. A zero quota
would reject every message and a zero fence observation budget would declare a
negative fence before any executor could be observed, so overloading zero as "no
limit" would make the two most dangerous misconfigurations indistinguishable from
the safest intent.

**Per-target quota SHALL NOT exceed relay-global quota in either dimension.**
`queued-envelopes-per-target-max` greater than `queued-envelopes-max`, or
`queued-bytes-per-target-max` greater than `queued-bytes-max`, SHALL fail
validation at load with a structured error naming both keys and both values. A
per-target limit above the global one is unreachable and therefore always a
configuration mistake.

**The undelivered-queue keys govern reporting only.** `undelivered-warning-ms`
and `undelivered-report-interval-ms` SHALL NOT influence any member's outcome,
release any admission quota, or alter scheduling. Their sole effect on elapse is
the emission of an inscription; see the `delivery-quiescence` capability's `Async
Delivery Observability` requirement for the emission rules.

`undelivered-warning-ms` SHALL default above the longest plausible agent turn, so
that a target legitimately mid-turn does not routinely produce warnings, and its
upper bound SHALL be permissive enough that an operator running long-horizon
agents can quiet it. Because zero is not permitted, the setting cannot be switched
off; raising it is the supported way to reduce its volume. It has no lower bound
tied to a turn length, because a short threshold produces noise rather than
incorrect behavior.

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

#### Scenario: Load explicit undelivered reporting settings

- **WHEN** `relay.toml` contains a `[delivery]` table setting
  `undelivered-warning-ms` and `undelivered-report-interval-ms` within their
  permitted ranges
- **THEN** relay startup uses those values for undelivered-queue reporting
- **AND** no member's outcome, quota, or scheduling position depends on either
  value

#### Scenario: Bound authorized execution

- **WHEN** `relay.toml` sets `[delivery].submission-timeout-ms` within the
  permitted range
- **THEN** the relay initiates that batch's generation fence and terminalizes no
  member at the bound
- **AND** every still-unresolved member is terminalized through the guard's
  evidence order at the fence verdict, not at the bound
- **AND** the setting is documented as an execution watchdog over the relay's own
  code, not as a judgement about target health

#### Scenario: Reject an out-of-range undelivered warning threshold

- **WHEN** `[delivery].undelivered-warning-ms` is below `60_000` or above
  `86_400_000`
- **THEN** relay startup fails with a structured error naming the key and the
  permitted range
- **AND** `agentmux check configuration` reports the same invalid artifact

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
