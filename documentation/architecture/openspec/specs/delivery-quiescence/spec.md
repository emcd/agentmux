# delivery-quiescence Specification

## Purpose

Send envelope, async queue lifecycle, terminal outcomes, ack semantics, and asynchronous terminal-outcome receipt.

## Requirements

### Requirement: JSON Send Envelope

The system SHALL inject messages as strict, pretty-printed JSON envelopes.

Each envelope SHALL include:

- `schema_version`
- `message_id` (globally unique identifier)
- `sender_session`
- `target_session` or broadcast marker
- `created_at`
- `body`

#### Scenario: Inject valid envelope

- **WHEN** a send request is accepted for delivery
- **THEN** the system renders a strict, pretty-printed JSON envelope
- **AND** injects the envelope into the target session via tmux

#### Scenario: Reject malformed envelope input fields

- **WHEN** required message fields are missing or invalid
- **THEN** the system rejects the request with a validation error

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
This is what makes an unbounded `Pending` wait safe: the members that wait are
exactly the ones for which nothing has been submitted, so no target-side effect is
outstanding for the duration of the wait, however long it runs.

The relay SHALL determine readiness from a **level-triggered**
`is_ready_for_handover` state read from the transport. Because a notification is
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

A target whose activity signal advanced across an observation pair is not
authorizable, even if the later observation happens to match the prompt-readiness
template. An advancing activity signal defers handover on its own.

The relay SHALL communicate the quiescence quiet period to transports that
perform target observation via the `DeliveryEnvelope.quiet_window: Duration`
field. The `prime_timeout_ms` and `readiness_timeout_ms` envelope fields are
removed, as are the per-coder configuration keys that populated them.

**How long a message may wait for a reachable target to become ready is not
bounded, by a transport timer or by any relay setting.** A `Pending` entry's *wait* ends
when its target becomes ready, when that target's transport is positively
observed torn down, when that transport has been continuously observed
`Unreachable` past `[delivery].unreachable-dwell-ms`, or when the relay shuts
down. It may also be resolved without having waited at all, by an immediate
refusal carrying its own evidence: a transport that cannot be constructed, a
target with no delivery path, a target member that cannot be resolved, or a batch
that does not transition. Those are refusals rather than expiries, and this list
is not the invariant.

**One exception is deliberate and SHALL be read as such: a fail-stopped worker
resolves every member it holds, including a `Pending` one whose target is
reachable.** A negative fence verdict means the relay could not establish that
the old generation stopped, so that worker submits nothing further and no
replacement generation may be elected for its target. A member held behind it can
therefore never be delivered by anything, and resolving it is the alternative to
stranding it for the life of the process. This exception is reachable through a
clock — the execution watchdog is anchored at authorization, so an *authorized*
member overrunning `[delivery].submission-timeout-ms` is what initiates the
fence — and the held member is resolved as a consequence of its worker becoming
unusable rather than as a judgement about its own wait. It is named here because
the requirement below would otherwise forbid it.

Subject to that exception, the invariant constrains what duration may do rather
than enumerating what may happen: elapsed waiting SHALL NOT resolve an entry whose
target is reachable,
because the length of a target's turn is not evidence about the target and no
bound the relay could pick would be anything but a guess about work it does not
control. Sustained unreachability is a different case, admitted deliberately and
bounded by the dwell: there, duration qualifies an observation repeatedly made
rather than substituting for one never made.

#### Scenario: Hand over after the target becomes ready

- **WHEN** a target's observable output remains unchanged for the configured
  quiet window
- **AND** the transport reports `is_ready_for_handover`
- **THEN** the relay authorizes the batch and the transport submits it

#### Scenario: Keep waiting while the target is active

- **WHEN** a target's output continues changing
- **THEN** the entry remains `Pending` and schedulable
- **AND** no terminal outcome is issued for it

#### Scenario: A settled non-prompt frame is not a failure

- **WHEN** a target is quiescent with the prompt frame absent
- **THEN** no terminal outcome is issued on that basis
- **AND** the entry remains `Pending`
- **BECAUSE** the inspected tail cannot distinguish a hung coder from a
  permission dialog, a compose box, or a coder working silently

#### Scenario: A continuously animating target keeps waiting

- **WHEN** a target's output advances on every observation without the
  prompt-readiness template ever matching, for arbitrarily long
- **THEN** the entry remains `Pending` and no terminal outcome is issued for it
- **AND** the relay emits undelivered-queue inscriptions for that target
- **BECAUSE** a target that is busy is a target that may still become ready, and
  the relay reports that condition rather than resolving it

#### Scenario: A ready target is authorized however long it took

- **WHEN** a target is observed prompt-ready after an arbitrarily long wait
- **AND** the target's activity signal did not advance across the observation
  pair
- **THEN** the batch is authorized
- **BECAUSE** reaching readiness late is the outcome the wait existed to obtain,
  and no elapsed duration disqualifies it

#### Scenario: An active target is not authorized on a momentary match

- **WHEN** the target's activity signal advanced across the observation pair
- **AND** the later observation happens to match the prompt-readiness template
- **THEN** no batch is authorized for it
- **AND** the entry remains `Pending`
- **BECAUSE** an advancing activity signal defers handover on its own

#### Scenario: A transport does not wait for readiness

- **WHEN** a transport receives an authorized envelope
- **THEN** it SHALL NOT wait on prompt readiness, target turn completion, target
  output, or an operator decision before submitting
- **AND** it starts exactly one immediate submission attempt

#### Scenario: Stale readiness yields an evidence-based outcome

- **WHEN** a transport's readiness state changes between the relay's check and
  authorization
- **AND** the resulting invocation is refused
- **THEN** the refused invocation's members resolve `not_submitted` or
  `submission_unknown` per the transport contract
- **AND** the relay SHALL NOT reclaim or retry them

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
- **AND** it states that such a target's messages wait without a duration bound
  rather than resolving on elapsed time, and that the undelivered-queue
  inscriptions are how an operator notices

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
no authorization SHALL occur across a raw barrier, nor younger work across older.

**The ordering guarantee is worker-enqueue linearization, not request arrival
order.** Mail and raw both reach a target through the same keyed worker, and the
order established is the order in which sends reach that worker's channel. A
request that reserves admission before another may still lose the race to enqueue
and be delivered second. Admission order and delivery order are therefore
distinct, and only the latter is guaranteed. Stating this precisely matters
because the weaker property is the one the implementation provides and the one a
test can hold it to.

**Cross-target scheduling fairness is deliberately out of scope.** Each target is
served by its own worker, and the relay does not arbitrate between targets. No
rotation, credit, or per-visit budget is specified, and none may be introduced
without first naming the resource being allocated and the fairness guarantee
being offered.

An earlier revision specified byte-budgeted round-robin with a configured
quantum. It is withdrawn, and the reason is recorded because the mistake is easy
to repeat. The quantum was required to be at least the largest permitted handover
byte component, while batch formation was already capped at that same component —
so one quantum always afforded at least one full batch, and the credit could
constrain only a second batch within one visit. Visits existed only because a
rotation existed. The budget was there to be fair within a rotation, and the
rotation was there to allocate the budget.

**Targets do contend, and that is not an argument for restoring it.** Tmux
targets in one bundle share a single tmux server and socket; ACP bootstrap enters
a shared blocking pool; and a transport whose write seam blocks can occupy a
delivery-runtime worker thread. None of these is measured by a global byte
quantum, which represents neither runtime occupancy nor channel slots nor
tmux-server capacity. A resource-grounded policy would be denominated per shared
resource and would state a throughput or fairness objective. No such objective is
required today. A transport that blocks the write seam is a contract violation to
be repaired at its source rather than a load to be scheduled around; see the
`transport-abstraction` capability's non-blocking handover requirement.

**No elapsed duration SHALL resolve a `Pending` entry whose target is
reachable.** A `Pending` entry leaves that state only by being authorized, by its
target's transport being positively observed torn down without replacement, by
that transport being continuously observed `Unreachable` for longer than
`[delivery].unreachable-dwell-ms`, or by graceful relay shutdown. The
unreachability case is specified in full by the `transport-abstraction`
capability, which owns the health axis; it is named here so this enumeration is
exhaustive.

That case is a concession to observation, **not an admission of delivery
timeouts.** The distinction is exact: a forbidden bound lets duration
*substitute* for an observation — nothing was seen, so after N seconds a verdict
is guessed. The dwell lets duration *qualify* an observation that was actually
made, repeatedly. Sustained unreachability is itself evidence; sustained busyness
is not.

**No configuration key SHALL bound how long a reachable target may leave an entry
`Pending`, and none may be introduced.** The length of a target's turn is not
evidence about the target, and such a bound would terminalize the entry and
release its quota — discarding a message that would have been delivered once the
target came back. The relay reports a long wait through undelivered-queue
inscriptions and never resolves one.

Consequently the relay guarantees that every accepted message resolves **at most
once**, not that every accepted message eventually resolves. Uniqueness is held by
the terminal transition; completeness is not claimed, and restoring it is the
`fetch`-cursor work tracked in `agentmux:todos/runtime/23` rather than a timer.

Scheduling and quota policy SHALL live in relay configuration rather than
`coders.toml`, because they are properties of the relay's own queue rather than of
any coder.

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

#### Scenario: A pending entry is never resolved by elapsed time

- **WHEN** an entry has been `Pending` for an arbitrarily long duration
- **AND** it has not been authorized, its target's transport has not been
  positively observed torn down, its target's transport has not been
  continuously `Unreachable` past `[delivery].unreachable-dwell-ms`, and the
  relay has not shut down
- **THEN** it remains `Pending`
- **AND** no terminal outcome is issued and no admission quota is released for it

#### Scenario: A long-waiting entry is still authorized when its target returns

- **WHEN** an entry has been `Pending` far longer than any transport's former
  readiness or prime bound
- **AND** its target then becomes ready
- **THEN** the batch is authorized and submitted normally
- **BECAUSE** the entry was never disqualified by how long it waited

#### Scenario: An unready target is not authorized

- **WHEN** a target has pending work but its transport does not report
  `is_ready_for_handover`
- **THEN** none of its entries are authorized
- **AND** they remain `Pending` with their admission quota still reserved
- **AND** no elapsed duration converts that unreadiness into an outcome

#### Scenario: Batch formation obeys both handover components

- **WHEN** a target's pending work would exceed either the envelope-count
  component or the canonical-payload-byte component of its transport's maximum
  handover dimensions
- **THEN** the handover stops at whichever component binds first
- **AND** the remainder stays `Pending` for a later handover

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
`failed`, `not_submitted`, `submission_unknown`, and `dropped_on_shutdown`. A
`delivered` outcome SHALL NOT produce a receipt; it is recorded per Async
Delivery Observability only. `PeerUnavailable` is a cross-relay outcome
reported synchronously on the send response, not a locally asynchronous
terminal outcome, and produces no receipt here.

Because no outcome is produced by elapsed waiting, a message queued for a target
that stays reachable but never becomes ready produces **no receipt at all** while
it waits. A target that goes continuously unreachable is the exception to the
elapsed-wait rule: its members resolve `not_submitted` past the dwell and receipt
normally, as members resolved by teardown or shutdown also do. The sender
is told at accept time that the message was `queued`, and learns nothing further
until the message resolves. This is deliberate: a receipt issued while an entry is
still waiting could only report that the relay had stopped waiting, which is a
fact about the relay rather than about the message.

`not_submitted` and `submission_unknown` are both non-delivered terminal
outcomes and SHALL produce receipts exactly as `dropped_on_shutdown` does. They
are not
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
  outcome (`failed`, `not_submitted`, `submission_unknown`, or
  `dropped_on_shutdown`)
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

#### Scenario: Deliver a torn-down transport receipt

- **WHEN** a queued message resolves `not_submitted` because its target's
  transport was positively observed torn down without replacement
- **AND** the original sender's session is routable
- **THEN** relay delivers a terminal-outcome receipt naming that `message_id`,
  target, and `not_submitted` to the sender
- **BECAUSE** nothing was authorized and the target is positively gone, so the
  relay can soundly state that the message was not delivered

#### Scenario: No receipt is produced while an entry waits

- **WHEN** a queued message has been `Pending` for an arbitrarily long duration
- **THEN** relay delivers no terminal-outcome receipt for it
- **BECAUSE** it has no terminal outcome, and a receipt reporting only that the
  relay was still waiting would state nothing about the message

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

The terminal-outcome inscription SHALL cover every locally asynchronous
terminal outcome: `delivered`, `failed`, `not_submitted`, `submission_unknown`,
and `dropped_on_shutdown`. `PeerUnavailable` is a cross-relay outcome reported
synchronously on the send response, not a locally asynchronous one, and is
outside this inscription's scope. This inscription SHALL be
recorded regardless of whether a terminal-outcome receipt is delivered to the
sender, so `relay.log` is a complete observability floor for terminal outcomes.

Recording the terminal outcome SHALL NOT depend on the sender's outcome
notification being deliverable. A closed or dropped notification path SHALL be
counted and recorded, and SHALL NOT prevent the entry's terminal transition or
the release of its admission quota.

A positively observed target exit or connection close SHALL be recorded as
**target-health observability**, not as a delivery outcome for an already
resolved member.

**Relay SHALL report its undelivered queue.** Because no entry is resolved by
elapsed waiting while its target stays reachable, a target that stops draining
without going unreachable accumulates `Pending` entries silently, and reporting is
the only thing that makes that condition visible. Two
emissions are required:

- **A periodic aggregate**, at the cadence of
  `[delivery].undelivered-report-interval-ms`, carrying the relay-global count of
  `Pending` entries and the canonical payload bytes reserved **by those `Pending`
  entries**, and a per-target breakdown for every target with at least one
  `Pending` entry. Both figures scope to `Pending` alone; bytes reserved by
  `Authorized` entries are excluded, so the count and the byte figure always
  describe the same set. The aggregate SHALL be
  **suppressed entirely when no entry is `Pending`**, so an idle relay emits
  nothing rather than a recurring zero.
- **A first-crossing warning per target**, emitted once when a target's oldest
  `Pending` entry first exceeds `[delivery].undelivered-warning-ms`, carrying the
  target, its `Pending` count, and its oldest entry's age.

The warning SHALL be deduplicated **per target, not per entry**. A target that has
already warned SHALL NOT warn again until its `Pending` queue empties, after which
a subsequent crossing SHALL warn again. Deduplicating per entry would emit one
inscription per queued message at the moment a backlogged target crosses, which is
the volume the threshold exists to control; and the condition an operator acts on
is that a target is not draining, which is a property of the target rather than of
any individual message.

**Neither emission SHALL affect delivery.** Crossing the warning threshold, and
any number of aggregate emissions, SHALL NOT resolve an entry, release admission
quota, alter scheduling order, or change any member's outcome. These are the only
duration-triggered mechanisms remaining on the `Pending` side, and they are sound
precisely because elapsing produces a record and nothing else.

#### Scenario: Report undelivered queue depth periodically

- **WHEN** at least one entry is `Pending` and the report interval elapses
- **THEN** relay writes an inscription carrying the relay-global `Pending` count
  and the bytes reserved by those `Pending` entries, and a per-target breakdown
- **AND** bytes reserved by `Authorized` entries are excluded from both figures
- **AND** it does so again on each subsequent interval while any entry is
  `Pending`

#### Scenario: Suppress the aggregate when nothing is pending

- **WHEN** the report interval elapses and no entry is `Pending`
- **THEN** relay writes no undelivered-queue aggregate inscription

#### Scenario: Warn once per target on first crossing

- **WHEN** a target's oldest `Pending` entry first exceeds
  `[delivery].undelivered-warning-ms`
- **THEN** relay writes one warning inscription naming that target, its `Pending`
  count, and its oldest entry's age

#### Scenario: A backlogged target warns once, not once per message

- **WHEN** a target has many `Pending` entries that cross the warning threshold
  together
- **THEN** relay writes exactly one warning inscription for that target
- **AND** the remaining entries are reflected only in that target's `Pending`
  count and in the periodic aggregate

#### Scenario: A target warns again after draining and re-accumulating

- **WHEN** a warned target's `Pending` queue empties
- **AND** it later accumulates a new entry that exceeds the warning threshold
- **THEN** relay writes a new warning inscription for that target

#### Scenario: Undelivered reporting does not resolve or reorder anything

- **WHEN** a target crosses the warning threshold and several aggregates are
  emitted while it remains backlogged
- **THEN** no entry resolves, no admission quota is released, and no target's
  scheduling position changes
- **BECAUSE** these emissions report a wait rather than adjudicating it

#### Scenario: Record queued async acceptance

- **WHEN** relay accepts an async target for queued delivery
- **THEN** relay writes an inscription event containing target session and
  message id with queued state

#### Scenario: Record terminal async outcome

- **WHEN** an async queued target reaches a terminal state (`delivered`,
  `failed`, `not_submitted`, `submission_unknown`, or `dropped_on_shutdown`)
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

Documentation SHALL state plainly that **no bound governs how long a delivery
waits for a reachable target to become ready**, on any transport, and that
`[delivery].unreachable-dwell-ms` is not that bound. It SHALL NOT direct
operators to configuration keys that do not exist, and SHALL NOT describe
per-coder timer keys that this change deletes.

Documentation SHALL describe `[delivery].submission-timeout-ms` as bounding the
relay's own supervised execution after authorization, and SHALL NOT present it as
a bound on how long a message may wait for a target. Conflating the two would
recreate, in the operator's mental model, exactly the bound the contract does not
have.

Queue growth SHALL be described accurately, including what is **not** guaranteed.
An `Authorized` entry always leaves the queue, because the authorization guard
terminalizes it. A `Pending` entry leaves the queue only on authorization, on a
positively observed transport teardown, on its transport being continuously
observed `Unreachable` past `[delivery].unreachable-dwell-ms`, or at graceful
shutdown — so a message queued for a target that stays **reachable but never
ready** occupies its admission quota indefinitely. Documentation SHALL state this
directly rather than implying that every queued message eventually resolves,
SHALL distinguish the reachable-but-unready case from the unreachable one so an
operator does not expect the dwell to rescue the former, SHALL point operators to
the undelivered-queue inscriptions as the way to observe it, and SHALL explain
that per-target admission quota is what bounds the consequence.

Documentation SHALL state the two bounds that genuinely do not exist: durability
across a relay crash, and completeness of resolution for `Pending` entries.

#### Scenario: Document the bounds that apply to async delivery

- **WHEN** operator-facing documentation is updated for async delivery mode
- **THEN** it states that no setting bounds how long a delivery waits for a
  target that is reachable but not ready, on any transport
- **AND** it states that `unreachable-dwell-ms` bounds only continuous
  unreachability, and does not rescue a reachable target that never becomes ready
- **AND** it describes `submission-timeout-ms` as bounding the relay's own
  post-authorization execution rather than the wait for a target
- **AND** it does not reference a `quiescence_timeout_ms`, a per-coder
  `prime-timeout-ms`, or a per-coder `readiness-timeout-ms` setting

#### Scenario: Document that a pending entry may never resolve

- **WHEN** operator-facing documentation describes queue growth
- **THEN** it states that a message queued for a target that stays reachable but
  never becomes ready remains queued and holds its admission quota indefinitely
- **AND** it distinguishes that case from a continuously unreachable target,
  whose members resolve past `[delivery].unreachable-dwell-ms`
- **AND** it names per-target admission quota as what bounds the consequence, and
  the undelivered-queue inscriptions as how to observe it
- **AND** it does not claim that every queued message eventually reaches a
  terminal outcome

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

**Authorization covers admitted members and nothing else.** Relay-originated work
holds no queue entry by design — a terminal-outcome receipt is created in response
to another member's resolution, reserves no quota, and has no `Pending` state to
leave — so there is nothing for the transition to act on. The relay SHALL attempt
authorization only for members holding an admission reservation, and a formed set
holding none SHALL proceed to submission without one. Absence of an authorization
that cannot exist SHALL NOT be treated as a refusal: a receipt refused on those
grounds deletes the only notice a sender receives that its message did not arrive,
and deletes it in exactly the case the sender most needs it.

This SHALL NOT be read as permission to submit an admitted member unauthorized.
Where a formed set mixes the two, authorization SHALL cover its admitted members
and the set SHALL be refused whole if that authorization fails. The distinction is
the reservation, not the message's origin, and it SHALL be read from the task
rather than inferred from the ledger — a terminal transition removes the entry it
resolves, so an absent entry cannot by itself distinguish work that was never
admitted from work another resolver already finished.

After authorization the relay SHALL NOT reclaim the message, SHALL NOT retry it,
and SHALL NOT assert non-delivery by inference. Positive evidence of
non-submission remains reportable.

**The relay's invocation of the transport is fallible.** The relay's admission
quota reserves count and bytes in the relay's own queue and reserves nothing about
a transport's channel, its live worker generation, or any target resource. A
post-authorization
refusal SHALL therefore be treated as a terminal evidence result, not a reclaim.

**The governing invariant:** no transition to `Authorized` SHALL occur unless an
owner capable of terminalizing and releasing every member of the batch is created
in the **same atomic operation**. Authorization either synchronously refuses or
synchronously starts one supervised submission executor. No `Authorized` batch
SHALL wait in a relay or transport staging queue; an authorized invocation
sitting behind an in-flight turn before partition is a post-authorization wait
wearing a queue's clothing.

Resolution SHALL be scoped precisely, because an indefinite `Pending` wait means
completeness does not hold for every accepted member. The three claims are
distinct and SHALL NOT be collapsed into a blanket "resolves exactly once":

- **Uniqueness** — any member that reaches a terminal state SHALL do so **exactly
  once**, in a surviving relay process, including when a transport, worker, or
  collector panics.
- **Bounded completeness for `Authorized` members** — every `Authorized` member
  SHALL reach a terminal state within `[delivery].submission-timeout-ms` plus
  twice `[delivery].fence-observation-timeout-ms`, on a positive and a negative
  fence verdict alike.
- **No completeness for `Pending` members** — a `Pending` member MAY never reach a
  terminal state while the relay and its target both remain live, and no mechanism
  SHALL manufacture one for it.

Uniqueness SHALL be enforced by a **relay-owned authorization guard** owned
outside every worker,
collector, and transport task. A keyed map plus a compare-and-set is not
sufficient on its own, because it cannot observe a detached thread, a worker-task
panic, a collector panic, or a generation replacement.

Guard identity SHALL be established in two atomic steps, because a packing unit
does not exist at authorization:

1. a guard is created at authorization bound to `(batch ID, member ID, attempt
   ID)`;
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

#### Mandatory post-authorization execution bound

Every trigger listed above is an *event*. An executor that remains alive and
blocked forever produces none of them: it does not unwind, does not close its
channel, does not exit, does not prompt a replacement, and does not reach
shutdown. Without a bound, such a batch would never resolve, its admission quota
would leak permanently, and its target's FIFO and raw barrier would stay blocked
— the same defect that put the guard in the core phase, on the other side of the
authorization line.

**The relay SHALL therefore bound post-authorization execution.** A batch's
execution SHALL be bounded by `[delivery].submission-timeout-ms`, anchored at
authorization. When that bound elapses and the batch has not fully resolved, the
relay SHALL **initiate the generation fence**, and SHALL NOT terminalize its
members at that moment.

**There is exactly one resolution cut, and it is the fence verdict.** Unit
evidence SHALL continue to be accepted throughout the bounded fence windows, and
every still-unresolved member SHALL be terminalized through the guard's evidence
order at the positive-or-negative verdict.

Terminalizing at the bound instead would destroy evidence the fence is about to
produce. A bound member with no record would win `submission_unknown`; if the
cooperative stop then halted it before any effect and recorded `NotSubmitted`,
the terminal CAS could no longer accept that stronger evidence, and the sender
would be told "this may have arrived" about a message the system had just proven
never left. One cut at the verdict preserves the evidence order rather than
racing it.

Evidence that arrives normally, before the bound elapses, SHALL still terminalize
its members as it does today. The cut governs what is *left* unresolved.

Total resolution therefore remains bounded, by `submission-timeout-ms` plus twice
the fence observation budget.

**This bound is not a reintroduction of the timers this change retires, and the
distinction is the whole basis of the change.** A retired timer concluded that
*the target had failed* because its screen did not change or its prompt did not
return — an inference from absence about a system the relay cannot see. This
bound states a fact about the relay's own supervised code: *our execution
exceeded the time we allow it, so we are stopping and recording that we do not
know.* It asserts nothing about target health, and it produces
`submission_unknown` rather than a failure, precisely because not knowing is what
actually happened.

It is an execution watchdog, and it SHALL be described as one wherever it is
documented, so a later reader does not mistake it for the class of timer this
change removed.

#### Scenario: A blocked executor is bounded rather than waiting forever

- **WHEN** an authorized batch's executor remains alive and blocked past
  `[delivery].submission-timeout-ms`
- **THEN** the generation fence is initiated
- **AND** no member is terminalized at that moment
- **AND** every still-unresolved member is terminalized through the guard's
  evidence order at the fence verdict

#### Scenario: Fence evidence still wins after the bound elapses

- **WHEN** the execution bound elapses and the fence's cooperative stop halts a
  bound member's unit before it produces any effect
- **AND** that unit records `NotSubmitted`
- **THEN** the member resolves `not_submitted` at the verdict
- **AND** it does not resolve `submission_unknown`
- **BECAUSE** the single resolution cut is the verdict, so evidence the fence
  produces is still admissible

#### Scenario: The execution bound does not override stronger evidence

- **WHEN** the execution bound elapses while one packing unit has already
  recorded `Submitted`
- **THEN** that unit's members resolve `delivered`
- **AND** only bound members lacking stronger evidence at the cut resolve
  `submission_unknown`

#### Scenario: Quota is released at terminalization, target barriers at the fence

- **WHEN** members are terminalized at the fence verdict
- **THEN** their admission quota and outcome-level barriers are released
- **AND** the target's FIFO, raw barrier, and generation replacement are released
  only on a **positive** verdict
- **AND** on a negative verdict other targets continue to progress while this one
  remains fail-stop

#### Scenario: The execution bound asserts nothing about the target

- **WHEN** the execution bound elapses
- **THEN** no member resolves to a failure spelling, and no target-health state
  is inferred
- **AND** a bound member lacking stronger evidence at the cut resolves
  `submission_unknown`
- **BECAUSE** the bound reports that the relay's own execution overran, not that
  the target is unhealthy

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

- **WHEN** the relay invokes a transport with an authorized envelope
- **AND** the transport refuses the invocation
- **THEN** the refused members resolve from evidence
- **AND** the relay does not return them to `Pending` and does not retry them

#### Scenario: A receipt is not refused for want of an authorization it cannot hold

- **GIVEN** a member resolves to a non-delivered outcome and its sender has a live
  delivery worker
- **WHEN** the relay submits the terminal-outcome receipt, which holds no
  admission reservation
- **THEN** authorization is not attempted for it
- **AND** the receipt is submitted to the sender's transport rather than resolved
  as unauthorized

#### Scenario: An admitted member is still authorized beside relay-originated work

- **WHEN** a formed set holds both an admitted member and relay-originated work
- **THEN** authorization covers the admitted member
- **AND** a failure to authorize it refuses the whole set rather than submitting
  either

#### Scenario: An authorized batch never waits in a staging queue

- **WHEN** a batch is authorized
- **THEN** the transport either synchronously refuses it or synchronously starts
  one supervised submission executor
- **AND** the invocation is not parked behind an in-flight turn before partition

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

**Shutdown budgets SHALL nest.** Graceful shutdown runs under one process-wide
**shutdown-work deadline**, and every bounded step on the shutdown path SHALL
size itself from what remains of it rather than from a duration configured in
isolation. A step SHALL reserve headroom for the steps that follow it, and a step
whose configured bound exceeds the remaining budget SHALL be cut down to fit
rather than allowed to overrun.

The shutdown-work deadline is **distinct from, and never later than, the
watchdog's forced exit.** It SHALL be established at the first of: the watchdog
observing the shutdown signal, or the first step to need a budget once shutdown
has been requested. The two differ by however long the watchdog has yet to
observe, so a deadline established by a step is *earlier* than the forced exit
rather than equal to it — which is the required direction. The specification
deliberately does **not** require them to coincide: a step that waited for the
watchdog to publish before computing a budget would make every shutdown depend on
that thread being scheduled promptly under exactly the load that makes shutdown
slow, and one that assumed they coincided would hand out a deadline later than
the exit it must precede.

The rule exists because the durations have no relationship otherwise:
`[delivery].fence-observation-timeout-ms` is operator-configurable and the fence
spends **two** of those windows, nested inside a delivery-worker wait and a
watchdog grace that neither knows nor validates against it. Without this rule,
raising a delivery timeout for an unrelated reason silently causes work behind
the fence to be lost when the process exits underneath it.

Consequently, a shutdown fence MAY be cut short by the deadline and return a
**negative verdict**. That verdict is a fail-safe, not a report of a transport
fault: the process is exiting, no replacement generation will be admitted, and
unresolved members terminalize through the guard's evidence order exactly as
they would on any other trigger.

**Resolving a member SHALL NOT depend on a step it does not require.** Members
that were never authorized and never handed to a transport SHALL resolve before
the shutdown fence runs, because nothing about their outcome depends on whether
a generation ceased. Ordering them after it made a guarantee that is
fence-independent hostage to fence duration.

#### Scenario: A shutdown fence cut short by the deadline still resolves every member

- **WHEN** the shutdown deadline leaves less time than the configured fence
  observation requires
- **THEN** the fence observes for the remaining budget rather than the configured
  duration
- **AND** a negative verdict resolves unresolved members through the evidence order
- **AND** no replacement generation is admitted, because the process is exiting

#### Scenario: Never-authorized members resolve before the fence

- **WHEN** relay shuts down gracefully with members still queued to a worker and
  never authorized
- **THEN** those members resolve `dropped_on_shutdown` before the generation
  fence begins
- **AND** their resolution does not depend on the fence's verdict or duration

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
