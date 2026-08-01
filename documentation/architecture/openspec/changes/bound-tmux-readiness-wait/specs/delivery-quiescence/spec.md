## MODIFIED Requirements

### Requirement: Quiescence-Gated Delivery

The system SHALL avoid injecting a message while target session output is
actively changing. Quiescence gating is transport-internal: each transport that
supports quiescence (Tmux today) SHALL wait for the target to become idle before
flushing its internal write buffer. The relay delivery worker SHALL NOT
orchestrate quiescence; it delivers writes via `mailw` and awaits outcome futures.

The relay SHALL communicate per-write quiescence bounds to the transport via
three `DeliveryEnvelope` fields:

- `quiet_window: Duration` — the quiet period before the transport
  declares the target ready to receive a flush group. Shared across all
  transports that perform quiescence waits.
- `prime_timeout_ms: Option<u64>` — generic prime-timeout bound that any
  prime-wait transport MAY consume. The relay populates this field from
  per-coder config (e.g. `[coders.<id>.tmux].prime-timeout-ms` for Tmux
  sessions; the ACP delivery-side timeout follow-up will populate the
  same field from `[coders.<id>.acp].prime-timeout-ms` for ACP sessions).
- `readiness_timeout_ms: Option<u64>` — the bound on the entire wait for the
  flush group, for transports whose readiness contract defines one. The relay
  SHALL populate it at envelope construction, `Some` for Tmux targets from
  `[coders.<id>.tmux].readiness-timeout-ms` and `None` for every other transport
  — Pty, ACP, UI, and pubsub — none of which has defined a readiness contract. A
  `None` value SHALL NOT be read as that transport being bounded by other means.

`[coders.<id>.tmux].readiness-timeout-ms` SHALL default to `900_000`
milliseconds when absent, and SHALL be validated to the inclusive range
`30_000..=3_600_000`. A value outside that range SHALL be rejected with a
structured configuration error at load. The default SHALL exceed the longest
plausible agent turn, since a target mid-turn is legitimately not ready and its
message must wait; the lower bound SHALL keep an operator from configuring a
value beneath a single turn, and the upper bound SHALL keep the setting from
re-creating an effectively unbounded wait.

The Tmux readiness wait SHALL be bounded. The bound SHALL cover the whole wait
for the flush group, including the prime window and any period of continuous
target activity — not merely the post-quiescence readiness wait. It SHALL be
anchored at the instant the flush group's wait begins, the same origin the prime
window uses, so that coalesced envelopes share the head envelope's clock.

No signal SHALL defer, extend, or suspend the readiness bound. A transport
observing that the target is mid-generation SHALL continue to withhold the flush
group from injection, and MAY continue to suppress terminal classification on
that basis, but SHALL NOT thereby outlive the bound. Once the bound has elapsed
the transport SHALL NOT continue waiting, and no wait it schedules SHALL extend
past it.

The bound SHALL NOT be expressed as a positional early return that pre-empts
other terminal checks. A transport SHALL evaluate every elapsed bound before
selecting an outcome, then apply precedence: when both the prime and readiness
bounds have elapsed in the same iteration, the prime timeout SHALL take
precedence, because it is the more specific diagnosis and is reached only when
an operator opted into it. Otherwise the earlier bound governs.

The readiness bound SHALL be the **unconditional termination guarantee** for a
Tmux post-quiescence wait: it applies regardless of what the pane shows and
regardless of whether a prime timeout is configured, and no signal defers it. It
is not the only terminal path — an opted-in prime timeout, relay shutdown, and a
positively observed probe or transport failure each remain terminal — but it is
the only one guaranteed to arrive, which is what makes the wait terminate.

No classification of pane content SHALL produce a terminal failure on a Tmux
wait. A settled non-prompt frame is produced by a hung coder, by a permission
dialog awaiting an operator, by a compose box holding typed input, and by a
coder working without terminal output; these are indistinguishable from the
inspected tail, so the absence of a prompt frame SHALL NOT be treated as
evidence that the target has failed.

Target activity advancing between observations remains a valid **positive**
signal on every transport and SHALL continue to suppress injection. **For Tmux**,
its absence SHALL NOT be treated as a signal of any kind: only a positively
observed terminal event — process death, a closed connection, a protocol error —
is sound evidence of failure, and an unchanged screen is not. Pty's retained
wedge classifier does infer failure from an unchanged screen; it is a
known-unsound exception carried until `agentmux:issues/relay/61` supplies a Pty
readiness bound, not a counterexample to this rule (see the
`transport-abstraction` capability's `Three-State Delivery Classifier`).

When the readiness bound elapses without delivery or a prime timeout having
fired, the transport SHALL resolve the flush group as `SendOutcome::Timeout` and
SHALL derive the reason code from the most recent observation. The reason is
diagnostic only; every arm is the same outcome:

| Most recent observation | `reason_code` |
|---|---|
| Prompt frame absent | `target_not_ready` |
| Inspected tail empty | `target_unresponsive` |
| Frame present, cursor away from its idle column | `pending_operator_input` |
| Target activity advancing across the observation pair | `target_never_settled` |

Classification precedence within one observation pair, highest first: activity
advancing, then an empty tail, then a cursor mismatch, then frame absence.

Outcome precedence when more than one outcome is available in the same
iteration, highest first: **delivery**, then the prime timeout, then the
readiness bound.

Delivery ranks first. A target observed prompt-ready in the same iteration in
which the readiness bound elapses SHALL be delivered to, not resolved as
`Timeout`. The bound exists to stop an unbounded wait, not to refuse a delivery
that is available: reaching readiness late is the outcome the wait was for, and
failing it there would discard a success on a technicality. This also preserves
the existing branch order, in which the prompt-ready check already precedes the
prime-timeout check.

The prime timeout outranks the readiness bound because it is the more specific
diagnosis — no observable output at all — and is reached only when an operator
opted into it.

A target suppressed as Busy is not prompt-ready for this purpose: activity
advancing across the observation pair defers delivery under the existing
pre-classification, and if the bound has elapsed the group resolves
`target_never_settled` rather than being delivered on a momentary match.

Reporting an expired readiness bound as non-delivery is sound for Tmux because
the Tmux transport injects into the pane only after its readiness wait, so an
expired bound provably precedes delivery. A transport that commits the message
before its readiness wait SHALL NOT report an expired bound as non-delivery;
that case is not covered by this requirement and is tracked as
`agentmux:issues/relay/61`.

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the transport flushes its write buffer and injects the pending messages

#### Scenario: Continue waiting for late quiescence within the readiness bound

- **WHEN** pane output continues changing
- **AND** the readiness bound has not elapsed
- **THEN** the transport keeps buffered writes pending
- **AND** flushes after a future quiescent window is observed

#### Scenario: Continuously advancing output still terminates

- **WHEN** a Tmux target's output advances on every observation without the
  prompt-readiness template ever matching
- **AND** the readiness bound elapses
- **THEN** the transport resolves the flush group as `Timeout` with
  `reason_code = "target_never_settled"`
- **BECAUSE** activity suppresses injection but not termination; a target that
  animates a non-prompt state indefinitely is otherwise indistinguishable from
  one that is about to finish

#### Scenario: Apply request prime timeout override on Tmux

- **WHEN** a Tmux-bound request carries a non-`None`
  `DeliveryEnvelope.prime_timeout_ms`
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** no observable output is produced before that timeout
- **THEN** the Tmux transport resolves the pending outcome futures
  with `SendOutcome::Timeout`
- **AND** records a `delivery_prime_timeout` inscription in relay
  diagnostics

#### Scenario: Prime timeout does not bound the post-quiescence wait

- **WHEN** the target pane output becomes quiescent
- **AND** the prompt-readiness template does not match
- **THEN** the transport SHALL NOT classify the flush group as `Timeout` solely
  on the basis of `prime_timeout_ms` elapsing
- **AND** the wait remains bounded by the readiness bound

#### Scenario: Prime timeout takes precedence when both bounds elapse together

- **WHEN** a finite prime timeout and the readiness bound have both elapsed in
  the same classification iteration
- **THEN** the flush group resolves with the prime-timeout outcome and reason
- **AND** the readiness reason is not reported for the same group

#### Scenario: Map Tmux prime timeout to transport envelope field

- **WHEN** a bundle member's `[coders.<id>.tmux].prime-timeout-ms` is
  set to a finite millisecond value
- **THEN** the relay attaches that value to the
  `DeliveryEnvelope.prime_timeout_ms` field at envelope construction
  time
- **AND** the Tmux transport uses it as the effective prime-window
  bound for the flush group

#### Scenario: Quiescence hints from head envelope govern the flush group

- **WHEN** the Tmux transport accumulates multiple envelopes with
  differing `quiet_window`, `prime_timeout_ms`, or `readiness_timeout_ms` values
  into one flush group
- **THEN** it uses the values from the first (head) envelope of the group as the
  effective bounds for the entire group
- **AND** a later envelope's bounds do not extend or shorten a wait already in
  progress for the group

#### Scenario: A transport with no readiness contract receives no bound

- **WHEN** the relay constructs a delivery envelope for a Pty, ACP, UI, or
  pubsub target
- **THEN** `readiness_timeout_ms` is `None`
- **AND** the transport's wait behavior is unchanged by this requirement

#### Scenario: Absent readiness timeout takes the default

- **WHEN** a bundle member's coder does not set
  `[coders.<id>.tmux].readiness-timeout-ms`
- **THEN** the effective bound for its Tmux writes is `900_000` milliseconds

#### Scenario: Accept the readiness timeout range boundaries

- **WHEN** a coder sets `[coders.<id>.tmux].readiness-timeout-ms` to `30_000`
  or to `3_600_000`
- **THEN** configuration load accepts the value
- **AND** the transport uses it as the effective bound

#### Scenario: Reject an out-of-range readiness timeout

- **WHEN** a coder sets `[coders.<id>.tmux].readiness-timeout-ms` below `30_000`
  or above `3_600_000`
- **THEN** configuration load fails with a structured error naming the key and
  the permitted range

#### Scenario: A prompt-ready target is delivered to despite a simultaneous expiry

- **WHEN** the readiness bound elapses in the same classification iteration in
  which the target is observed prompt-ready
- **AND** the target's activity signal did not advance across the observation
  pair
- **THEN** the flush group is injected and resolves as `Delivered`
- **AND** it is not resolved as `Timeout`
- **BECAUSE** reaching readiness, even late, is the outcome the wait existed to
  obtain; the bound exists to stop waiting forever, not to refuse a success that
  is already in hand

#### Scenario: An active target is not delivered to on a momentary match at expiry

- **WHEN** the readiness bound elapses
- **AND** the target's activity signal advanced across the observation pair
- **AND** the post-sleep observation happens to match the prompt-readiness
  template
- **THEN** the flush group resolves as `Timeout` with
  `reason_code = "target_never_settled"`
- **AND** no message is injected
- **BECAUSE** the Busy pre-classification already defers delivery on a momentary
  match, and an elapsed bound resolves the group rather than granting the match
  it was denied

#### Scenario: A settled non-prompt frame is not a failure before the bound

- **WHEN** a Tmux target's pane is quiescent with the prompt frame absent
- **AND** the readiness bound has not elapsed
- **THEN** no terminal outcome is issued for the flush group
- **AND** the group remains pending
- **BECAUSE** the inspected tail cannot distinguish a hung coder from a
  permission dialog, a compose box, or a coder working silently, so absence of a
  prompt frame is not evidence of failure

#### Scenario: A settled non-prompt frame resolves at the bound

- **WHEN** the readiness bound elapses
- **AND** the most recent observation shows an absent prompt frame
- **THEN** the flush group resolves as `Timeout` with
  `reason_code = "target_not_ready"`

### Requirement: Async Queue Growth Risk Disclosure

The system SHALL document the bounds that apply to async queueing, and SHALL NOT
describe bounds that do not exist.

Documentation SHALL describe the per-coder Tmux readiness bound as the setting
that governs how long a Tmux delivery may wait for a target, alongside the
optional prime timeout. It SHALL NOT direct operators to configuration keys that
do not exist.

Queue growth SHALL be described accurately per transport rather than as a single
unqualified risk. Tmux entries leave the queue when their delivery resolves,
which the readiness bound guarantees happens. Transports without a readiness
contract retain the unbounded-growth risk, and documentation SHALL say which
transports those are rather than implying the risk is universal or resolved.

#### Scenario: Document the bounds that apply to async delivery

- **WHEN** operator-facing documentation is updated for async delivery mode
- **THEN** it describes the per-coder Tmux readiness bound as the setting that
  governs how long a Tmux delivery may wait for a target
- **AND** it names the transports that remain unbounded rather than describing
  unbounded growth as a universal property
- **AND** it does not reference a `quiescence_timeout_ms` setting
