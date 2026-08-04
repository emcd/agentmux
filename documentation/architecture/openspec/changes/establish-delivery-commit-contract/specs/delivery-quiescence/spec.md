## MODIFIED Requirements

### Requirement: Quiescence-Gated Delivery

The system SHALL avoid injecting a message while target session output is
actively changing. Quiescence gating is **relay-owned**: the relay observes a
target's readiness and holds the message in its own queue until handing it over
is useful. A transport SHALL NOT wait for target readiness, and SHALL NOT decide
that a delivery failed from what a target displays or from how long it has been
quiet.

**All target-readiness and quiescence waiting happens before authorization**,
while the entry is `Pending`: prompt-readiness matching, quiescence observation,
and — on ACP — completion and operator-choice resolution of an **older** turn.
This is what makes residency expiry sound: the members that wait are exactly the
ones for which nothing has been submitted.

The relay SHALL determine readiness from a **level-triggered**
`can_accept_handover` state read from the transport. Because a notification is
only an edge, the relay SHALL subscribe before checking or re-check after
subscribing, SHALL poll at a bounded cadence as a backstop, and SHALL re-read
the level on every notification, admission, and completion. Readiness is
advisory: it says handing over is useful, not that it is permitted. A reading
that goes stale between the check and authorization SHALL produce an
evidence-based terminal outcome per the transport contract, not a lost or
misreported message.

No classification of target content SHALL produce a terminal failure on any
transport. A settled non-prompt frame is produced by a hung coder, by a
permission dialog awaiting an operator, by a compose box holding typed input,
and by a coder working without terminal output; these are indistinguishable from
the inspected tail, so the absence of a prompt frame SHALL NOT be treated as
evidence that the target has failed.

Target activity advancing between observations remains a valid **positive**
signal on every transport and SHALL continue to suppress handover. Its absence
SHALL NOT be treated as a signal of any kind: only a positively observed
terminal event — process death, a closed connection, a protocol error — is
sound evidence of failure, and an unchanged screen is not. This rule now holds
without exception; the Pty wedge classifier that was previously carried as a
known-unsound exception is removed by this change.

**Authorization outranks residency expiry.** When a target becomes authorizable
in the same scheduling iteration in which an entry's residency elapses, the relay
SHALL authorize rather than expire. Residency exists to stop an unbounded wait,
not to refuse a handover that is available: reaching readiness late is the
outcome the wait was for. A target whose activity signal advanced in that
iteration is not authorizable, so it expires rather than being authorized on a
momentary match.

The relay SHALL communicate the quiescence quiet period to transports that
perform target observation via the `DeliveryEnvelope.quiet_window: Duration`
field. The `prime_timeout_ms` and `readiness_timeout_ms` envelope fields are
removed, as are the per-coder configuration keys that populated them. How long a
message may wait is a property of the relay's patience and is governed by
residency, not by a per-transport timer.

#### Scenario: Hand over after the target becomes ready

- **WHEN** a target's observable output remains unchanged for the configured
  quiet window
- **AND** the transport reports `can_accept_handover`
- **THEN** the relay authorizes the batch and the transport submits it

#### Scenario: Keep waiting while the target is active

- **WHEN** a target's output continues changing
- **AND** the entry's residency has not elapsed
- **THEN** the entry remains `Pending` and schedulable
- **AND** no terminal outcome is issued for it

#### Scenario: A settled non-prompt frame is not a failure

- **WHEN** a target is quiescent with the prompt frame absent
- **THEN** no terminal outcome is issued on that basis
- **AND** the entry remains `Pending`
- **BECAUSE** the inspected tail cannot distinguish a hung coder from a
  permission dialog, a compose box, or a coder working silently

#### Scenario: A continuously animating target still terminates

- **WHEN** a target's output advances on every observation without the
  prompt-readiness template ever matching
- **AND** the entry's residency elapses
- **THEN** the entry resolves `expired`
- **BECAUSE** activity suppresses handover but not termination, and residency is
  a statement about the relay's own patience rather than about the target

#### Scenario: A ready target is authorized despite a simultaneous expiry

- **WHEN** an entry's residency elapses in the same scheduling iteration in which
  its target is observed prompt-ready
- **AND** the target's activity signal did not advance across the observation
  pair
- **THEN** the batch is authorized and the entry is not resolved `expired`
- **BECAUSE** reaching readiness, even late, is the outcome the wait existed to
  obtain

#### Scenario: An active target is not authorized on a momentary match at expiry

- **WHEN** an entry's residency elapses
- **AND** the target's activity signal advanced across the observation pair
- **AND** the later observation happens to match the prompt-readiness template
- **THEN** the entry resolves `expired`
- **AND** no batch is authorized for it
- **BECAUSE** an advancing activity signal already defers handover, and an
  elapsed residency resolves the entry rather than granting the match it was
  denied

#### Scenario: A transport does not wait for readiness

- **WHEN** a transport receives an authorized batch
- **THEN** it SHALL NOT wait on prompt readiness, target turn completion, target
  output, or an operator decision before submitting
- **AND** it starts exactly one immediate submission attempt

#### Scenario: Stale readiness yields an evidence-based outcome

- **WHEN** a transport's readiness state changes between the relay's check and
  authorization
- **AND** the resulting invocation is refused
- **THEN** the batch's members resolve `not_submitted` or `submission_unknown`
  per the transport contract
- **AND** the relay SHALL NOT reclaim or retry the batch

### Requirement: Delivery Results Without ACK Protocol

Relay SHALL use asynchronous acceptance responses and SHALL NOT support
synchronous completion responses.

An accepted send request SHALL return immediately with per-target `outcome =
queued`. Relay SHALL NOT block the caller waiting for delivery completion.

Admission SHALL atomically reserve the entry's admission quota — envelope count
and canonical payload bytes, per target and relay-global — **before** `queued`
is returned. A request that cannot be admitted SHALL be rejected at admission
rather than queued.

An envelope whose canonical payload size alone exceeds the target transport's
maximum handover dimensions SHALL be rejected at admission rather than queued
unsendable.

A `Pubsub` target SHALL be rejected **synchronously at admission** with the
existing not-implemented error, before anything is queued or authorized. It
produces no terminal outcome and no receipt, because nothing was accepted. Work
SHALL NOT be authorized merely to discover the stub.

#### Scenario: Report accepted async delivery

- **WHEN** relay accepts a send request for one or more targets
- **THEN** the immediate result marks those targets as `queued`
- **AND** does not wait for final delivery outcome before responding

#### Scenario: Return no-op completion for zero effective targets

- **WHEN** sender exclusion and target resolution produce zero effective
  recipients
- **THEN** relay returns an immediate no-op response without validation error
- **AND** response contains zero per-target results

#### Scenario: Reject at admission when quota is exhausted

- **WHEN** admitting a message would exceed the per-target or relay-global
  admission quota in either envelope count or canonical payload bytes
- **THEN** relay rejects the request with a structured error
- **AND** no queue entry is created and no quota is reserved

#### Scenario: Reject an envelope larger than the transport can ever accept

- **WHEN** an envelope's canonical payload size exceeds the target transport's
  declared maximum handover dimensions
- **THEN** relay rejects the request at admission
- **BECAUSE** queueing it would park a message that no partition could ever carry

#### Scenario: Reject a Pubsub target synchronously

- **WHEN** a send request names a `Pubsub` target
- **THEN** relay returns the not-implemented error at admission
- **AND** no queue entry is created, no batch is authorized, and no receipt is
  produced

### Requirement: Async Queue Lifecycle and Ordering

Relay SHALL maintain an in-memory pending queue. The queue SHALL be non-durable.

Each queue entry SHALL carry an explicit state — `Pending`, `Authorized`, or
`Terminal` — and each authorization SHALL carry a **stable attempt ID**. These
are required now because retrofitting them onto a durable queue later is
expensive; they do not imply that durability exists.

Relay SHALL preserve FIFO ordering per target session and SHALL NOT deduplicate
or coalesce queued messages. Mail and raw are variants of one per-target FIFO:
no authorization SHALL occur across a raw barrier, nor younger work across older,
except where the emergency raw mode defined in the `transport-contracts`
capability applies.

Scheduling across targets SHALL be **deficit round-robin**. Maximum handover
dimensions have two components — envelope count and canonical payload bytes — and
the two are used for different purposes, so each rule below names which:

- **cost unit** — canonical payload bytes, the same unit as admission quota, so
  one accounting serves both. Envelope count is not a scheduling cost;
- **quantum** — a relay-configured byte value per rotation visit, compared
  against the **canonical-payload-byte component** of every registered
  transport's maximum handover dimensions. It SHALL be greater than or equal to
  the largest such byte component. Configuring it lower SHALL be a validation
  error at load. The count component is not compared against the quantum, since
  the quantum is denominated in bytes;
- **batch formation** — a batch SHALL satisfy **both** components of its target
  transport's maximum handover dimensions: no more envelopes than the count
  maximum, and no more canonical payload bytes than the byte maximum. Whichever
  binds first stops the batch. A batch SHALL additionally not exceed the
  visiting target's remaining quantum plus deficit;
- **debit timing** — a target's deficit SHALL be debited by a batch's canonical
  payload bytes **at authorization**, in the same atomic operation as the
  `Pending` → `Authorized` transition. Debiting at admission would charge work
  that may never be authorized; debiting at resolution would let a target be
  visited repeatedly while its earlier batches are still in flight;
- **deficit accrual** — a per-target counter accruing the unused quantum each
  visit, **capped at one quantum**, so an idle target cannot bank credit and then
  monopolise a rotation;
- **eligible rotation** — only targets with pending work whose transport reports
  `can_accept_handover` are visited; ineligible targets are skipped without
  accruing deficit;
- **revalidation** — when the set of registered transports changes, or a
  registered transport's declared maximum handover dimensions change, the relay
  SHALL revalidate the configured quantum against the new largest byte component.
  If the quantum no longer satisfies the constraint, the relay SHALL refuse to
  register that transport and SHALL record the refusal, rather than silently
  admitting a transport whose handover it cannot schedule.

Because the quantum is at least the largest permitted byte component, and
admission rejects an envelope exceeding its transport's maximum handover
dimensions, every admissible item fits within one quantum. There is no
oversized-item case.

Residency governs `Pending` entries only. When an entry's residency elapses
before it is authorized, it SHALL resolve `expired`. Residency expiry is a
**pre-commit** outcome: it is a statement about the relay's own patience, never
about the target's health, and it SHALL NOT fire once a message is authorized.

Residency and scheduling policy SHALL live in relay configuration rather than
`coders.toml`, because they are properties of the relay's patience rather than of
any coder. Per-target residency overrides are excluded from this change.

#### Scenario: Drop pending async queue on relay restart

- **WHEN** relay exits or restarts before delivering queued async targets
- **THEN** pending async entries are discarded
- **AND** they are not recovered from durable storage

#### Scenario: Preserve per-target FIFO ordering

- **WHEN** multiple async messages are queued for the same target session
- **THEN** relay authorizes them in enqueue order for that target

#### Scenario: Do not deduplicate queued async messages

- **WHEN** queued async messages have identical body content or same target set
- **THEN** relay treats them as distinct queue entries
- **AND** attempts each entry independently

#### Scenario: Expire a pending entry at its residency bound

- **WHEN** an entry has been `Pending` for its full residency
- **AND** it has not been authorized
- **THEN** it resolves `expired`
- **AND** its admission quota is released

#### Scenario: Residency does not apply to an authorized entry

- **WHEN** an entry has transitioned to `Authorized`
- **AND** its residency would otherwise have elapsed
- **THEN** no residency expiry occurs for it
- **AND** it resolves only from submission evidence

#### Scenario: Skip an unready target without accruing deficit

- **WHEN** a target has pending work but its transport does not report
  `can_accept_handover`
- **THEN** the rotation skips it
- **AND** its deficit counter does not increase

#### Scenario: Cap accrued deficit at one quantum

- **WHEN** a target is skipped across several consecutive rotations
- **THEN** its deficit counter does not exceed one quantum
- **AND** it cannot consume more than one quantum's worth of a later rotation

#### Scenario: Reject a quantum smaller than a registered byte maximum

- **WHEN** the configured scheduling quantum is less than the canonical-payload-
  byte component of any registered transport's maximum handover dimensions
- **THEN** configuration load fails with a structured error naming the key, the
  configured value, and the transport whose byte component exceeds it

#### Scenario: Batch formation obeys both handover components

- **WHEN** a target's pending work would exceed either the envelope-count
  component or the canonical-payload-byte component of its transport's maximum
  handover dimensions
- **THEN** the batch stops at whichever component binds first
- **AND** the remainder stays `Pending` for a later rotation

#### Scenario: Debit deficit at authorization

- **WHEN** a batch transitions from `Pending` to `Authorized`
- **THEN** the target's deficit is debited by that batch's canonical payload
  bytes in the same atomic operation
- **AND** the debit does not wait for the batch to resolve

#### Scenario: Refuse a transport whose handover the quantum cannot cover

- **WHEN** a transport registers, or changes its declared maximum handover
  dimensions, such that the configured quantum is below its byte component
- **THEN** the relay refuses to register that transport and records the refusal
- **AND** it does not silently admit a transport whose handover it cannot
  schedule

### Requirement: Asynchronous Terminal-Outcome Receipt

When a queued message resolves to a non-delivered terminal outcome, relay SHALL
deliver a terminal-outcome receipt back to the original sender, out of band from
the accept-time response. The receipt SHALL be a relay-originated envelope
addressed to the sender and delivered through the sender's own transport via the
existing delivery pipeline, the same way any message reaches that session (a
Tmux pane, an ACP turn, a Pty write, or a UI stream frame). The receipt SHALL
carry the original `message_id`, the delivery target, the terminal outcome, and
any `reason_code`, so the sender can correlate it to the `queued` result it
received at accept time.

Receipts SHALL be delivered for non-delivered terminal outcomes only:
`not_submitted`, `submission_unknown`, `expired`, `transport_unavailable`, and
`dropped_on_shutdown`. A `delivered` outcome SHALL NOT produce a receipt; it is
recorded per Async Delivery Observability only.

`not_submitted` and `submission_unknown` are both non-delivered terminal
outcomes and SHALL produce receipts exactly as `expired` does. They are not
interchangeable: `not_submitted` asserts non-delivery on positive evidence that
no side effect occurred, while `submission_unknown` states that side effects
cannot be excluded. A receipt SHALL NOT collapse them into a single spelling,
because the sender's reasonable next action differs.

A terminal-outcome receipt SHALL be relay/system-originated and SHALL NOT be
attributed to a peer principal, so a recipient can distinguish it from inbound
peer traffic.

A terminal-outcome receipt is itself a delivery and SHALL NOT produce a receipt
of its own; receipts are non-recursive. A receipt's own terminal outcome SHALL be
recorded per Async Delivery Observability and go no further.

Receipt delivery SHALL be best-effort. If the sender session is not routable at
terminal-resolution time, relay SHALL drop the receipt. Relay SHALL NOT persist,
queue indefinitely, or retry a dropped receipt; deferred delivery is out of
scope. The underlying terminal outcome SHALL still be recorded per Async Delivery
Observability regardless of whether the receipt is delivered.

`queued` SHALL denote async acceptance for delivery only. Relay SHALL NOT present
`queued` as a terminal `delivered`/success outcome, and the terminal outcome
SHALL be the authoritative result for a queued message.

#### Scenario: Deliver a non-delivered outcome receipt through the sender's transport

- **WHEN** a queued message to a target resolves as a non-delivered terminal
  outcome (`not_submitted`, `submission_unknown`, `expired`,
  `transport_unavailable`, or `dropped_on_shutdown`)
- **AND** the original sender's session is routable
- **THEN** relay delivers a terminal-outcome receipt to the sender through the
  sender's own transport
- **AND** the receipt carries the original `message_id`, the delivery target, the
  terminal outcome, and any `reason_code`

#### Scenario: Distinguish absence of evidence from evidence of absence

- **WHEN** one queued message resolves `not_submitted` and another resolves
  `submission_unknown`
- **THEN** each receipt names its own outcome
- **AND** neither is reported using the other's spelling
- **BECAUSE** the first asserts the message did not arrive and the second states
  that it may have

#### Scenario: Deliver a residency expiry receipt

- **WHEN** a queued message resolves `expired` at its residency bound
- **AND** the original sender's session is routable
- **THEN** relay delivers a terminal-outcome receipt naming that `message_id`,
  target, and `expired` to the sender
- **BECAUSE** nothing was authorized, so the relay can soundly state that the
  message was not delivered

#### Scenario: No receipt for a delivered outcome

- **WHEN** a queued message resolves as `delivered`
- **THEN** relay does not deliver a terminal-outcome receipt to the sender
- **AND** records the `delivered` outcome per Async Delivery Observability

#### Scenario: Drop receipt when the sender is not routable

- **WHEN** a queued message resolves to a non-delivered terminal outcome
- **AND** the original sender's session is not routable at resolution time
- **THEN** relay drops the receipt without persisting or retrying it
- **AND** relay still records the terminal outcome per Async Delivery
  Observability

#### Scenario: Receipts are not recursive

- **WHEN** a terminal-outcome receipt delivered to a sender itself reaches a
  terminal outcome
- **THEN** relay does not deliver a receipt for the receipt
- **AND** records the receipt's own terminal outcome per Async Delivery
  Observability

#### Scenario: Queued is not a terminal success signal

- **WHEN** a target is accepted for async delivery
- **THEN** the accept-time per-target result is `queued`
- **AND** `queued` is not presented as a terminal `delivered`/success outcome
- **AND** the authoritative outcome is the terminal outcome, delivered as a
  receipt when non-delivered

### Requirement: Async Delivery Observability

Relay SHALL emit inscriptions for async queue lifecycle transitions.

The terminal-outcome inscription SHALL cover every terminal outcome:
`delivered`, `not_submitted`, `submission_unknown`, `expired`,
`transport_unavailable`, and `dropped_on_shutdown`. This inscription SHALL be
recorded regardless of whether a terminal-outcome receipt is delivered to the
sender, so `relay.log` is a complete observability floor for terminal outcomes.

Recording the terminal outcome SHALL NOT depend on the sender's outcome
notification being deliverable. A closed or dropped notification path SHALL be
counted and recorded, and SHALL NOT prevent the entry's terminal transition or
the release of its admission quota.

A positively observed target exit or connection close SHALL be recorded as
**target-health observability**, not as a delivery outcome for an already
resolved member.

#### Scenario: Record queued async acceptance

- **WHEN** relay accepts an async target for queued delivery
- **THEN** relay writes an inscription event containing target session and
  message id with queued state

#### Scenario: Record terminal async outcome

- **WHEN** an async queued target reaches a terminal state (`delivered`,
  `not_submitted`, `submission_unknown`, `expired`, `transport_unavailable`, or
  `dropped_on_shutdown`)
- **THEN** relay writes an inscription event containing target session,
  message id, and terminal outcome

#### Scenario: Record terminal outcome even when no receipt is delivered

- **WHEN** an async queued target reaches a terminal state
- **AND** no terminal-outcome receipt is delivered (the outcome is `delivered`,
  or the sender is not routable)
- **THEN** relay still writes the terminal-outcome inscription

#### Scenario: An unreachable notification path does not strand an entry

- **WHEN** an entry reaches a terminal outcome
- **AND** the sender's outcome notification path is closed
- **THEN** the entry still transitions to `Terminal` and releases its admission
  quota
- **AND** relay records the undeliverable notification

#### Scenario: Record a post-resolution target exit as health, not delivery

- **WHEN** a target process exits or its connection closes after a member has
  already resolved
- **THEN** relay records the event as target-health observability
- **AND** does not issue a second delivery outcome for that member

### Requirement: Async Queue Growth Risk Disclosure

The system SHALL document the bounds that apply to async queueing, and SHALL NOT
describe bounds that do not exist.

Documentation SHALL describe relay-side residency as the setting that governs how
long any delivery may wait for a target, and SHALL describe it as applying
uniformly to every transport. It SHALL NOT direct operators to configuration keys
that do not exist, and SHALL NOT describe per-coder timer keys that this change
deletes.

Queue growth SHALL be described accurately. Every entry leaves the queue when it
reaches a terminal outcome, which residency guarantees for `Pending` entries and
the authorization guard guarantees for `Authorized` ones. Documentation SHALL
state the one bound that genuinely does not exist: durability across a relay
crash.

#### Scenario: Document the bounds that apply to async delivery

- **WHEN** operator-facing documentation is updated for async delivery mode
- **THEN** it describes relay-side residency as the setting that governs how long
  a delivery may wait for a target
- **AND** it describes that bound as applying to every transport rather than
  naming transports that remain unbounded
- **AND** it does not reference a `quiescence_timeout_ms`, a per-coder
  `prime-timeout-ms`, or a per-coder `readiness-timeout-ms` setting

### Requirement: Quiescence Documentation

The system SHALL document quiescence constraints and known interference
patterns for users configuring agent sessions.

Documentation SHALL describe quiescence observation as relay-owned, so an
operator reading it does not look for per-transport waiting behavior that no
longer exists.

#### Scenario: Document dynamic output caveat

- **WHEN** project documentation is generated for the relay capability
- **THEN** it includes a warning that continuously changing output sources
  (for example clock-style statusline content) can prevent quiescence
  detection from succeeding
- **AND** it states that such a target's messages resolve `expired` at residency
  rather than waiting indefinitely

## ADDED Requirements

### Requirement: Delivery Authorization and Terminal Guard

Delivery SHALL be modelled as four distinct events, not one:

| Event | Owner | Atomic | Reversible |
|---|---|---|---|
| **Admission** — accept into the queue, reserve quota, return `queued` | relay | yes | yes |
| **Authorization** — `Pending` → `Authorized` on the queue entry | relay | **yes — the linearization point** | **no** |
| **Submission** — one packing unit produces a target-side effect | transport | per unit | no |
| **Resolution** — each member resolves once, from recorded facts | transport → relay | per member | no |

**Authorization is a relay-local state transition on the relay's own queue
entry.** It is not a call, not a handshake, and does not depend on the transport
observing anything. Cancellation competes only with this transition, and it
competes relay-locally. Authorizing a batch SHALL authorize every member in it
atomically; there is no partially-authorized batch.

After authorization the relay SHALL NOT reclaim the message, SHALL NOT retry it,
and SHALL NOT assert non-delivery by inference. Positive evidence of
non-submission remains reportable.

**The relay's invocation of the transport is fallible.** Relay residency reserves
count and bytes in the relay's own queue and reserves nothing about a transport's
channel, its live worker generation, or any target resource. A post-authorization
refusal SHALL therefore be treated as a terminal evidence result, not a reclaim.

**The governing invariant:** no transition to `Authorized` SHALL occur unless an
owner capable of terminalizing and releasing every member of the batch is created
in the **same atomic operation**. Authorization either synchronously refuses or
synchronously starts one supervised submission executor. No `Authorized` batch
SHALL wait in a relay or transport staging queue; a batch sitting behind an
in-flight turn before partition is a post-authorization wait wearing a queue's
clothing.

Every accepted member SHALL resolve **exactly once** in a surviving relay
process, including when a transport, worker, or collector panics. This SHALL be
enforced by a **relay-owned authorization guard** owned outside every worker,
collector, and transport task. A keyed map plus a compare-and-set is not
sufficient on its own, because it cannot observe a detached thread, a worker-task
panic, a collector panic, or a generation replacement.

Guard identity SHALL be established in two atomic steps, because a packing unit
does not exist at authorization:

1. a guard is created at authorization bound to `(batch ID, member ID, attempt
   ID, transport generation)`;
2. when the transport records its partition, each guard is atomically bound to
   its `PackingUnit ID`.

A pre-partition refusal or panic therefore terminalizes through the
batch/member-level guard without requiring a unit ID that was never assigned.

The guard SHALL:

- consume normal evidence through **one atomic non-terminal → terminal
  transition**, so duplicate completions converge rather than racing;
- carry keys in collectors rather than granting them ownership of resolution;
- terminalize any still-unresolved member on unwind, channel closure, supervised
  task or thread exit, generation replacement, and graceful shutdown;
- leave `Pending` entries untouched, so they remain schedulable or take a
  pre-commit policy outcome.

#### Guard resolution order

Whenever the guard terminalizes a member that has not already reached a terminal
outcome, it SHALL select that outcome by the following order, first match
winning:

1. the member's packing unit has an **immutable evidence record** → derive the
   outcome from that record (`Submitted` → `delivered`, `NotSubmitted` →
   `not_submitted`, `SubmissionUnknown` → `submission_unknown`);
2. the member was **never bound to a packing unit** → `not_submitted`, because
   the partition is recorded before the first target-side effect, so nothing
   could have been submitted;
3. otherwise → `submission_unknown`.

**Lifecycle context determines *when* the guard resolves a member, never *which*
outcome it receives.** Unwind, channel closure, task or thread exit, generation
replacement, and graceful shutdown are all triggers for the same evidence order.
No requirement SHALL specify an outcome for a member on the basis of the
lifecycle event that prompted its resolution, because doing so would report
`submission_unknown` for members the system can positively prove were never
submitted.

A fence that stops a submission before any target-side effect SHALL record
`NotSubmitted` as that unit's evidence, so it resolves through step 1 rather
than needing a rule of its own.

**Admission quota SHALL be released by the guard's terminal transition**, and by
nothing else. Releasing it anywhere other than the single terminal transition
permits a double release on any path that attempts termination twice, which the
fault paths routinely produce.

Relay SHALL never retry an authorized batch. The guarantee is **at most one
relay-authorized injection attempt**, not at-most-once delivery: transports do
not deduplicate attempt IDs, so a stronger claim would be false. A message that
did not arrive, reported honestly, leaves the decision with the sender.

#### Scenario: Create the owner atomically with authorization

- **WHEN** a batch transitions from `Pending` to `Authorized`
- **THEN** an owner capable of terminalizing and releasing every member is
  created in the same atomic operation
- **AND** no window exists in which an `Authorized` member has no owner

#### Scenario: Resolve exactly once under a worker panic

- **WHEN** the submission executor for an authorized batch panics
- **THEN** every member of that batch reaches exactly one terminal outcome
- **AND** each member's admission quota is released exactly once

#### Scenario: An unbound member resolves not_submitted whatever the trigger

- **WHEN** the guard terminalizes a member that was never bound to a packing unit
- **AND** the trigger is a panic, a channel closure, a generation replacement, or
  graceful shutdown
- **THEN** the member resolves `not_submitted` in every case
- **AND** the lifecycle event does not change the outcome

#### Scenario: A recorded unit outcome outranks the lifecycle trigger

- **WHEN** a generation is replaced while one of its units has already recorded
  `Submitted`
- **THEN** that unit's members resolve `delivered`
- **AND** they are not downgraded to `submission_unknown` because a replacement
  occurred

#### Scenario: Resolve exactly once under a collector panic

- **WHEN** a collector panics after a unit's evidence is recorded but before the
  members are resolved from it
- **THEN** every member of that unit resolves from the recorded evidence
- **AND** no member is left without a terminal outcome

#### Scenario: Duplicate terminalization converges

- **WHEN** two paths attempt to terminalize the same member
- **THEN** exactly one transition occurs
- **AND** the admission quota is released exactly once
- **AND** the losing attempt does not alter the recorded outcome

#### Scenario: A refused invocation is terminal, not a reclaim

- **WHEN** the relay invokes a transport with an authorized batch
- **AND** the transport refuses the invocation
- **THEN** the batch's members resolve from evidence
- **AND** the relay does not return them to `Pending` and does not retry them

#### Scenario: An authorized batch never waits in a staging queue

- **WHEN** a batch is authorized
- **THEN** the transport either synchronously refuses it or synchronously starts
  one supervised submission executor
- **AND** the batch is not parked behind an in-flight turn before partition

#### Scenario: Never retry an authorized batch

- **WHEN** an authorized batch resolves `submission_unknown`
- **THEN** relay does not re-authorize, re-invoke, or duplicate it
- **AND** the sender receives a terminal-outcome receipt naming the uncertainty

### Requirement: In-Process Delivery Recovery Scope

The guarantees in this capability SHALL hold for a **surviving relay process and
graceful shutdown** only. This SHALL be stated as a limitation in operator-facing
documentation rather than implied.

An abrupt relay crash loses pending work, submission evidence, outcomes, and
sender notification alike. No abstraction reconciles them after the fact, and
this change does not claim to.

**Recovery behavior SHALL be specified only where it exists.** In-process
recovery is real and is specified: when a per-target worker or transport is torn
down and respawned within a surviving relay process, `Pending` entries SHALL be
rescheduled to the new generation, and `Authorized` entries SHALL **never** be
re-invoked — they resolve through the guard's evidence order.
Process-startup recovery is **not** specified, because nothing persists across a
process boundary.

On graceful shutdown, still-`Pending` relay-owned members SHALL resolve
`dropped_on_shutdown`. `Authorized` members SHALL NOT resolve
`dropped_on_shutdown`; they resolve through the guard's evidence order.

Neither respawn nor shutdown SHALL select an outcome for an `Authorized` member.
Both are triggers; the evidence order chooses.

#### Scenario: Reschedule pending entries to a new generation

- **WHEN** a transport generation is torn down and replaced within a surviving
  relay process
- **THEN** its `Pending` entries are rescheduled to the new generation
- **AND** they retain their position in the per-target FIFO

#### Scenario: Never re-invoke an authorized entry after respawn

- **WHEN** a transport generation is replaced while it holds `Authorized` entries
- **THEN** those entries resolve through the guard's evidence order
- **AND** they are not submitted to the replacement generation

#### Scenario: Separate pending and authorized members at shutdown

- **WHEN** relay shuts down gracefully with a mix of `Pending` and `Authorized`
  members
- **THEN** the `Pending` members resolve `dropped_on_shutdown`
- **AND** the `Authorized` members resolve through the guard's evidence order,
  never `dropped_on_shutdown`
- **AND** an `Authorized` member never bound to a packing unit resolves
  `not_submitted` rather than `submission_unknown`

#### Scenario: State the crash-recovery limitation

- **WHEN** operator-facing documentation describes delivery guarantees
- **THEN** it states that they hold for a surviving relay process and graceful
  shutdown only
- **AND** it does not describe process-startup recovery behavior
