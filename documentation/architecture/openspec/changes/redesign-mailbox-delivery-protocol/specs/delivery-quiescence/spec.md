## RENAMED Requirements

- FROM: `### Requirement: Delivery Results Without ACK Protocol`
- TO: `### Requirement: Delivery Results Without Synchronous Completion`
- FROM: `### Requirement: Async Queue Lifecycle and Ordering`
- TO: `### Requirement: Mailbox Ordering and Cursor Lifecycle`
- FROM: `### Requirement: Delivery Authorization and Terminal Guard`
- TO: `### Requirement: Delivery Guard and Acknowledgment Terminalization`

## MODIFIED Requirements

### Requirement: Quiescence-Gated Delivery

The system SHALL avoid injecting a message while target session output is
actively changing. Quiescence gating is **transport-owned**: the relay holds
custody of the message in its mailbox and applies no readiness gate of its
own before making an entry available to `peek`. A transport SHALL observe its
own target's readiness before choosing to write what it peeked, and SHALL NOT
report a delivery failure from what a target displays or from how long it has
been quiet.

**Mailbox availability is unconditional once admitted.** An admitted entry is
visible to `peek` immediately; there is no separate authorization step and no
relay-side readiness check gating that visibility. A transport that is busy,
or whose target is not ready, simply does not call `peek`, or calls it and
writes nothing — either way the entry stays queued, in order, for the next
attempt. This replaces the prior `Pending`-while-waiting model: waiting is
now the transport's own choice not to consume, not a relay-tracked state.

No classification of target content SHALL produce a terminal failure on any
transport. A settled non-prompt frame is produced by a hung coder, by a
permission dialog awaiting an operator, by a compose box holding typed input,
and by a coder working without terminal output; these are indistinguishable
from the inspected tail, so the absence of a prompt frame SHALL NOT be
treated as evidence that the target has failed.

Target activity advancing between observations remains a valid **positive**
signal on every transport and SHALL continue to defer a transport's own
decision to write. Its absence SHALL NOT be treated as a signal of any kind:
only a positively observed terminal event — process death, a closed
connection, a protocol error — is sound evidence of failure, and an unchanged
screen is not.

**How long an entry may sit unpeeked, or peeked-but-unacked, for a reachable
target is not bounded**, by a transport timer or by any relay setting. An
entry leaves the mailbox only by being acknowledged, by its target's
transport being positively observed torn down, by that transport being
continuously observed `Unreachable` past `[delivery].unreachable-dwell-ms`,
or by graceful relay shutdown.

**One exception is deliberate and SHALL be read as such: a fail-stopped
worker resolves every member it holds, including a queued one whose target
is reachable.** A negative fence verdict means the relay could not establish
that the old generation stopped, so no replacement generation may be elected
for its target and nothing further will ever peek its mailbox. A member held
behind it can therefore never be delivered by anything, and resolving it is
the alternative to stranding it for the life of the process.

Subject to that exception, elapsed waiting SHALL NOT resolve an entry whose
target is reachable, because the length of a target's turn is not evidence
about the target and no bound the relay could pick would be anything but a
guess about work it does not control. Sustained unreachability is admitted
deliberately and bounded by the dwell, as a repeated observation rather than
a substitute for one never made.

#### Scenario: An entry sits queued while the target is active

- **WHEN** a target's output continues changing
- **THEN** the transport does not write what it may have peeked
- **AND** the entry remains queued and unacked
- **AND** no terminal outcome is issued for it

#### Scenario: A settled non-prompt frame is not a failure

- **WHEN** a target is quiescent with the prompt frame absent
- **THEN** no terminal outcome is issued on that basis
- **AND** the entry remains queued
- **BECAUSE** the inspected tail cannot distinguish a hung coder from a
  permission dialog, a compose box, or a coder working silently

#### Scenario: A continuously animating target keeps entries queued

- **WHEN** a target's output advances on every observation without the
  prompt-readiness template ever matching, for arbitrarily long
- **THEN** its entries remain queued and no terminal outcome is issued for
  them
- **AND** the relay emits undelivered-mailbox inscriptions for that target
- **BECAUSE** a target that is busy is a target that may still become ready,
  and the relay reports that condition rather than resolving it

#### Scenario: A ready target is peeked and written however long it took

- **WHEN** a target is observed prompt-ready after an arbitrarily long wait
- **THEN** the transport peeks its mailbox and writes what it peeked
- **BECAUSE** reaching readiness late is the outcome the wait existed to
  obtain, and no elapsed duration disqualified the entry

#### Scenario: A transport does not wait once it writes

- **WHEN** a transport has decided to write a peeked entry
- **THEN** it does not additionally wait on prompt readiness, target turn
  completion, target output, or an operator decision before submitting
- **AND** it starts exactly one immediate submission attempt per packing unit

### Requirement: Quiescence Documentation

The system SHALL document quiescence constraints and known interference
patterns for users configuring agent sessions.

Documentation SHALL describe quiescence observation as transport-owned, so an
operator reading it understands that the relay applies no readiness gate of
its own and that an unpeeked or unacked entry is not evidence of a relay-side
problem.

#### Scenario: Document dynamic output caveat

- **WHEN** project documentation is generated for the relay capability
- **THEN** it includes a warning that continuously changing output sources
  (for example clock-style statusline content) can prevent a transport's own
  quiescence detection from succeeding
- **AND** it states that such a target's messages wait without a duration
  bound rather than resolving on elapsed time, and that the undelivered-
  mailbox inscriptions are how an operator notices

### Requirement: Delivery Results Without Synchronous Completion

Relay SHALL use asynchronous acceptance responses and SHALL NOT support
synchronous completion responses.

An accepted send request SHALL return immediately with per-target `outcome =
queued`. Relay SHALL NOT block the caller waiting for delivery completion.
`queued` denotes async acceptance into the mailbox only; it is not a claim
about acknowledgment.

Admission SHALL atomically reserve the entry's admission quota — envelope
count and canonical payload bytes, per target and relay-global — **before**
`queued` is returned. A request that cannot be admitted SHALL be rejected at
admission rather than queued.

An envelope whose canonical payload size alone exceeds the target transport's
maximum peek dimensions SHALL be rejected at admission rather than queued
unservable.

A `Pubsub` target SHALL be rejected **synchronously at admission** with the
existing not-implemented error, before anything is queued. It produces no
terminal outcome and no receipt, because nothing was accepted.

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
- **AND** no mailbox entry is created and no quota is reserved

#### Scenario: Reject an envelope larger than the transport can ever accept

- **WHEN** an envelope's canonical payload size exceeds the target
  transport's declared maximum peek dimensions
- **THEN** relay rejects the request at admission
- **BECAUSE** queueing it would park a message that no peek could ever carry

#### Scenario: Reject a Pubsub target synchronously

- **WHEN** a send request names a `Pubsub` target
- **THEN** relay returns the not-implemented error at admission
- **AND** no mailbox entry is created and no receipt is produced

### Requirement: Mailbox Ordering and Cursor Lifecycle

Relay SHALL maintain an in-memory per-target mailbox. The mailbox SHALL be
non-durable.

Each mailbox entry SHALL carry exactly two states — `queued` or `terminal` —
and an entry's own sequence number is its stable identity; there is no
separate attempt ID, because acknowledgment is idempotent per entry rather
than keyed to a submission attempt. There is no `Authorized` state and no
event that transitions an entry independent of acknowledgment.

Relay SHALL preserve FIFO ordering per target session and SHALL NOT
deduplicate or coalesce queued entries. Mail and raw are variants of one
per-target ordered mailbox: `peek` SHALL NOT return mail past an unpeeked raw
entry, and a raw entry at the head of the mailbox SHALL be returned alone.

**The ordering guarantee is enqueue linearization into the mailbox, not
request arrival order.** Mail and raw both reach a target's mailbox through
the same keyed admission path, and the order established is the order in
which sends reach that mailbox. A request that reserves admission before
another may still lose the race to enqueue and be peeked second. Admission
order and mailbox order are therefore distinct, and only the latter is
guaranteed.

**Cross-target scheduling fairness is deliberately out of scope.** Each
target's mailbox is independent, and the relay does not arbitrate between
targets by any credit, quantum, or rotation. A transport chooses when to
peek its own target's mailbox; the relay does not schedule that choice.

**No elapsed duration SHALL resolve a `queued` entry whose target is
reachable.** A `queued` entry leaves that state only by being acknowledged,
by its target's transport being positively observed torn down without
replacement, by that transport being continuously observed `Unreachable` for
longer than `[delivery].unreachable-dwell-ms`, or by graceful relay
shutdown. The unreachability case is specified in full by the
`transport-abstraction` capability, which owns the health axis; it is named
here so this enumeration is exhaustive.

Consequently the relay guarantees that every accepted message resolves **at
most once relative to acknowledgment bookkeeping** — an entry is terminalized
exactly once no matter how many times it is acknowledged — but does not
guarantee that every accepted message is ever acknowledged, and does not
guarantee **exactly-once delivery to the target**: at-least-once is the
accepted contract, and a transport that writes an entry and dies before its
`ack` commits produces a duplicate on replay, not a lost message.

Scheduling and quota policy SHALL live in relay configuration rather than
`coders.toml`, because they are properties of the relay's own mailbox rather
than of any coder.

#### Scenario: Drop mailbox contents on relay restart

- **WHEN** relay exits or restarts before every mailbox entry is acknowledged
- **THEN** unacknowledged entries are discarded
- **AND** they are not recovered from durable storage

#### Scenario: Preserve per-target FIFO ordering

- **WHEN** multiple async messages are queued for the same target session
- **THEN** `peek` returns them in enqueue order for that target

#### Scenario: Do not deduplicate queued async messages

- **WHEN** queued async messages have identical body content or same target
  set
- **THEN** relay treats them as distinct mailbox entries
- **AND** each is peeked and acknowledged independently

#### Scenario: A queued entry is never resolved by elapsed time

- **WHEN** an entry has been queued for an arbitrarily long duration
- **AND** it has not been acknowledged, its target's transport has not been
  positively observed torn down, its target's transport has not been
  continuously `Unreachable` past `[delivery].unreachable-dwell-ms`, and the
  relay has not shut down
- **THEN** it remains `queued`
- **AND** no terminal outcome is issued and no admission quota is released
  for it

#### Scenario: A raw entry is peeked only as a singleton

- **WHEN** a target's mailbox head is a raw-kind entry
- **THEN** `peek` returns exactly that one entry
- **AND** no mail entry behind it is included in the same `peek` response

#### Scenario: A duplicate acknowledgment is a no-op

- **WHEN** an `ack` names a sequence number at or behind the target's current
  cursor
- **THEN** the relay applies no further state change for that entry
- **AND** does not release its admission quota a second time

### Requirement: Asynchronous Terminal-Outcome Receipt

Relay SHALL deliver a terminal-outcome receipt back to the original sender
when a mailbox entry resolves to a non-delivered terminal outcome, out of
band from the accept-time response. The receipt SHALL be a relay-originated
envelope addressed to the sender and delivered through the sender's own
mailbox, the same way any message reaches that session. The receipt SHALL
carry the original `message_id`, the delivery target, the terminal outcome,
and any `reason_code`, so the sender can correlate it to the `queued` result
it received at accept time.

Receipts SHALL be delivered for non-delivered terminal outcomes only:
`failed`, `not_submitted`, `submission_unknown`, and `dropped_on_shutdown`. A
`delivered` outcome SHALL NOT produce a receipt; it is recorded per Async
Delivery Observability only. `PeerUnavailable` is a cross-relay outcome
reported synchronously on the send response, not a locally asynchronous
terminal outcome, and produces no receipt here.

Because no outcome is produced by elapsed waiting, a message queued for a
target that stays reachable but is never peeked, or is peeked but never
written, produces **no receipt at all** while it waits. A target that goes
continuously unreachable is the exception to the elapsed-wait rule: its
members resolve `not_submitted` past the dwell and receipt normally, as
members resolved by teardown or shutdown also do.

`not_submitted` and `submission_unknown` are both non-delivered terminal
outcomes and SHALL produce receipts exactly as `dropped_on_shutdown` does.
They are not interchangeable: `not_submitted` asserts non-delivery on
positive evidence that no side effect occurred, while `submission_unknown`
states that side effects cannot be excluded.

A terminal-outcome receipt SHALL be relay/system-originated and SHALL NOT be
attributed to a peer principal. A terminal-outcome receipt is itself a
delivery and SHALL NOT produce a receipt of its own; receipts are
non-recursive.

Receipt delivery SHALL be best-effort. If the sender session is not routable
at terminal-resolution time, relay SHALL drop the receipt without
persisting, queueing indefinitely, or retrying it. The underlying terminal
outcome SHALL still be recorded per Async Delivery Observability regardless
of whether the receipt is delivered.

#### Scenario: Deliver a non-delivered outcome receipt through the sender's mailbox

- **WHEN** a queued message to a target resolves as a non-delivered terminal
  outcome (`failed`, `not_submitted`, `submission_unknown`, or
  `dropped_on_shutdown`)
- **AND** the original sender's session is routable
- **THEN** relay delivers a terminal-outcome receipt to the sender's mailbox
- **AND** the receipt carries the original `message_id`, the delivery
  target, the terminal outcome, and any `reason_code`

#### Scenario: No receipt is produced while an entry waits

- **WHEN** a queued message has been queued for an arbitrarily long duration
- **THEN** relay delivers no terminal-outcome receipt for it
- **BECAUSE** it has no terminal outcome, and a receipt reporting only that
  the relay was still waiting would state nothing about the message

#### Scenario: Drop receipt when the sender is not routable

- **WHEN** a queued message resolves to a non-delivered terminal outcome
- **AND** the original sender's session is not routable at resolution time
- **THEN** relay drops the receipt without persisting or retrying it
- **AND** relay still records the terminal outcome per Async Delivery
  Observability

#### Scenario: Distinguish absence of evidence from evidence of absence

- **WHEN** one queued message resolves `not_submitted` and another resolves
  `submission_unknown`
- **THEN** each receipt names its own outcome
- **AND** neither is reported using the other's spelling
- **BECAUSE** the first asserts the message did not arrive and the second
  states that it may have

#### Scenario: Deliver a torn-down transport receipt

- **WHEN** a queued message resolves `not_submitted` because its target's
  transport was positively observed torn down without replacement
- **AND** the original sender's session is routable
- **THEN** relay delivers a terminal-outcome receipt naming that
  `message_id`, target, and `not_submitted` to the sender
- **BECAUSE** nothing was submitted and the target is positively gone, so
  the relay can soundly state that the message was not delivered

#### Scenario: No receipt for a delivered outcome

- **WHEN** a queued message resolves as `delivered`
- **THEN** relay does not deliver a terminal-outcome receipt to the sender
- **AND** records the `delivered` outcome per Async Delivery Observability

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

Relay SHALL emit inscriptions for mailbox lifecycle transitions.

The terminal-outcome inscription SHALL cover every locally asynchronous
terminal outcome: `delivered`, `failed`, `not_submitted`,
`submission_unknown`, and `dropped_on_shutdown`. This inscription SHALL be
recorded regardless of whether a terminal-outcome receipt is delivered to
the sender.

Recording the terminal outcome SHALL NOT depend on the sender's outcome
notification being deliverable.

A positively observed target exit or connection close SHALL be recorded as
**target-health observability**, not as a delivery outcome for an already
resolved member.

**Relay SHALL report its undelivered mailbox contents.** Because no entry is
resolved by elapsed waiting while its target stays reachable, a target that
stops draining without going unreachable accumulates `queued` entries
silently, and reporting is the only thing that makes that condition visible.
Two emissions are required:

- **A periodic aggregate**, at the cadence of
  `[delivery].undelivered-report-interval-ms`, carrying the relay-global
  count of `queued` entries and the canonical payload bytes reserved by
  those entries, and a per-target breakdown for every target with at least
  one `queued` entry. The aggregate SHALL be **suppressed entirely when no
  entry is `queued`**, so an idle relay emits nothing rather than a
  recurring zero.
- **A first-crossing warning per target**, emitted once when a target's
  oldest `queued` entry first exceeds `[delivery].undelivered-warning-ms`,
  carrying the target, its `queued` count, and its oldest entry's age.

The warning SHALL be deduplicated **per target, not per entry**. A target
that has already warned SHALL NOT warn again until its mailbox empties,
after which a subsequent crossing SHALL warn again.

**Neither emission SHALL affect delivery.** Crossing the warning threshold,
and any number of aggregate emissions, SHALL NOT resolve an entry, release
admission quota, alter mailbox order, or change any member's outcome.

#### Scenario: Report undelivered mailbox depth periodically

- **WHEN** at least one entry is `queued` and the report interval elapses
- **THEN** relay writes an inscription carrying the relay-global `queued`
  count and the bytes reserved by those entries, and a per-target breakdown
- **AND** it does so again on each subsequent interval while any entry is
  `queued`

#### Scenario: Suppress the aggregate when nothing is queued

- **WHEN** the report interval elapses and no entry is `queued`
- **THEN** relay writes no undelivered-mailbox aggregate inscription

#### Scenario: Warn once per target on first crossing

- **WHEN** a target's oldest `queued` entry first exceeds
  `[delivery].undelivered-warning-ms`
- **THEN** relay writes one warning inscription naming that target, its
  `queued` count, and its oldest entry's age

#### Scenario: A backlogged target warns once, not once per message

- **WHEN** a target has many `queued` entries that cross the warning
  threshold together
- **THEN** relay writes exactly one warning inscription for that target
- **AND** the remaining entries are reflected only in that target's `queued`
  count and in the periodic aggregate

#### Scenario: A target warns again after draining and re-accumulating

- **WHEN** a warned target's mailbox empties
- **AND** it later accumulates a new entry that exceeds the warning
  threshold
- **THEN** relay writes a new warning inscription for that target

#### Scenario: Undelivered reporting does not resolve or reorder anything

- **WHEN** a target crosses the warning threshold and several aggregates are
  emitted while it remains backlogged
- **THEN** no entry resolves, no admission quota is released, and no
  target's mailbox order changes
- **BECAUSE** these emissions report a wait rather than adjudicating it

#### Scenario: Record queued async acceptance

- **WHEN** relay accepts an async target for queued delivery
- **THEN** relay writes an inscription event containing target session and
  message id with `queued` state

#### Scenario: Record terminal async outcome

- **WHEN** a mailbox entry reaches a terminal state (`delivered`, `failed`,
  `not_submitted`, `submission_unknown`, or `dropped_on_shutdown`)
- **THEN** relay writes an inscription event containing target session,
  message id, and terminal outcome

#### Scenario: Record terminal outcome even when no receipt is delivered

- **WHEN** a mailbox entry reaches a terminal state
- **AND** no terminal-outcome receipt is delivered (the outcome is
  `delivered`, or the sender is not routable)
- **THEN** relay still writes the terminal-outcome inscription

#### Scenario: An unreachable notification path does not strand an entry

- **WHEN** an entry reaches a terminal outcome
- **AND** the sender's outcome notification path is closed
- **THEN** the entry still transitions to `terminal` and releases its
  admission quota
- **AND** relay records the undeliverable notification

#### Scenario: Record a post-resolution target exit as health, not delivery

- **WHEN** a target process exits or its connection closes after a member has
  already resolved
- **THEN** relay records the event as target-health observability
- **AND** does not issue a second delivery outcome for that member

### Requirement: Async Queue Growth Risk Disclosure

The system SHALL document the bounds that apply to mailbox queueing, and
SHALL NOT describe bounds that do not exist.

Documentation SHALL state plainly that **no bound governs how long an entry
waits for a reachable target to be peeked and written**, on any transport,
and that `[delivery].unreachable-dwell-ms` is not that bound.

Documentation SHALL describe `[delivery].submission-timeout-ms` as bounding
the relay's own supervised execution once a submission is underway, and
SHALL NOT present it as a bound on how long a message may wait to be peeked.

Queue growth SHALL be described accurately, including what is **not**
guaranteed. A mailbox entry leaves the mailbox only on acknowledgment, on a
positively observed transport teardown, on its transport being continuously
observed `Unreachable` past `[delivery].unreachable-dwell-ms`, or at
graceful shutdown — so a message queued for a target that stays **reachable
but never peeked or never written** occupies its admission quota
indefinitely. Documentation SHALL state this directly, SHALL distinguish the
reachable-but-unconsumed case from the unreachable one, SHALL point
operators to the undelivered-mailbox inscriptions as the way to observe it,
and SHALL explain that per-target admission quota is what bounds the
consequence.

Documentation SHALL state the two bounds that genuinely do not exist:
durability across a relay crash, and completeness of resolution for `queued`
entries.

#### Scenario: Document the bounds that apply to async delivery

- **WHEN** operator-facing documentation is updated for async delivery mode
- **THEN** it states that no setting bounds how long an entry waits to be
  peeked and written by a reachable target, on any transport
- **AND** it states that `unreachable-dwell-ms` bounds only continuous
  unreachability, and does not rescue a reachable target that never consumes
- **AND** it describes `submission-timeout-ms` as bounding the relay's own
  post-write execution rather than the wait to be peeked

#### Scenario: Document that a queued entry may never resolve

- **WHEN** operator-facing documentation describes mailbox growth
- **THEN** it states that a message queued for a target that stays reachable
  but never peeks or never writes remains queued and holds its admission
  quota indefinitely
- **AND** it distinguishes that case from a continuously unreachable target
- **AND** it names per-target admission quota as what bounds the
  consequence, and the undelivered-mailbox inscriptions as how to observe it

### Requirement: Delivery Guard and Acknowledgment Terminalization

Delivery SHALL be modelled as three distinct events, not four:

| Event | Owner | Atomic | Reversible |
|---|---|---|---|
| **Admission** — accept into the mailbox, reserve quota, return `queued` | relay | yes | yes |
| **Submission** — one packing unit produces a target-side effect | transport | per unit | no |
| **Acknowledgment** — advance the cursor and terminalize the covered members from supplied evidence | transport → relay | per `ack` call, per member within it | no |

There is no separate Authorization event. An admitted entry is visible to
`peek` unconditionally; committing to it happens only when a transport calls
`ack`, and that call is itself the linearization point — the relay treats an
`ack` for a still-active generation as final for the members it covers.

**Acknowledgment is a relay-local state transition on the relay's own
mailbox entries.** It is not permission-seeking; the transport has already
written by the time it calls `ack`. Cancellation (a generation being
revoked) competes only with this transition, and it competes relay-locally
under the same single lock, per the `Consumer Generation Ownership and
Replacement` and `Revocation Serialized Against In-Flight Acknowledgment`
requirements.

**Acknowledgment covers acknowledged members and nothing else.**
Relay-originated work (a terminal-outcome receipt) holds no mailbox
reservation by design, so there is nothing for `ack` to act on for it; it is
authorized implicitly by having no reservation to fail.

After acknowledgment the relay SHALL NOT reclaim the message, SHALL NOT
retry it, and SHALL NOT assert non-delivery by inference. Positive evidence
of non-submission remains reportable through the guard's evidence order
below.

**The relay's own bookkeeping is fallible only insofar as the transport's
supplied evidence is fallible.** The relay's admission quota reserves count
and bytes in the relay's own mailbox and reserves nothing about a
transport's channel, its live delivery-loop executor, or any target
resource.

Resolution SHALL be scoped precisely, because an indefinite `queued` wait
means completeness does not hold for every accepted member. The three claims
are distinct and SHALL NOT be collapsed into a blanket "resolves exactly
once":

- **Uniqueness** — any member that reaches a terminal state SHALL do so
  **exactly once**, in a surviving relay process, including when a
  transport's delivery-loop executor, or the relay's own evidence collector,
  panics.
- **Bounded completeness once submission has begun** — once a packing unit
  has produced a target-side effect, its members SHALL reach a terminal
  state within `[delivery].submission-timeout-ms` plus twice
  `[delivery].fence-observation-timeout-ms`, on a positive and a negative
  fence verdict alike.
- **No completeness for `queued` members that have not been submitted** — a
  `queued` member MAY never reach a terminal state while the relay and its
  target both remain live, and no mechanism SHALL manufacture one for it.

Uniqueness SHALL be enforced by a **relay-owned guard** owned outside every
transport delivery-loop executor. A keyed map plus a compare-and-set is not
sufficient on its own, because it cannot observe a detached executor, a
delivery-loop panic, an evidence-collector panic, or a generation
replacement.

Guard identity SHALL be a mailbox entry's own sequence number; because
acknowledgment is idempotent per entry (see `Mailbox Ordering and Cursor
Lifecycle`), no separate attempt identifier is required. When a transport's
`ack` names a packing unit's evidence, each covered entry's guard is bound to
that `PackingUnit ID` in the same atomic step that records the evidence.

The guard SHALL:

- consume normal evidence through **one atomic non-terminal → terminal
  transition**, so duplicate acknowledgments converge rather than racing;
- terminalize any still-unresolved member on unwind, channel closure,
  delivery-loop executor exit, generation replacement, and graceful
  shutdown;
- leave `queued` entries untouched, so they remain peekable by whatever
  generation is current.

#### Guard resolution order

Whenever the guard terminalizes a member that has not already reached a
terminal outcome, it SHALL select that outcome by the following order, first
match winning:

1. the member's packing unit has an **immutable evidence record** → derive
   the outcome from that record (`Submitted` → `delivered`, `NotSubmitted` →
   `not_submitted`, `SubmissionUnknown` → `submission_unknown`);
2. the member was **never bound to a packing unit** → `not_submitted`,
   because binding happens before the first target-side effect, so nothing
   could have been submitted;
3. otherwise → `submission_unknown`.

**Lifecycle context determines *when* the guard resolves a member, never
*which* outcome it receives.** Unwind, channel closure, executor exit,
generation replacement, and graceful shutdown are all triggers for the same
evidence order.

#### Mandatory post-submission execution bound

Once a packing unit has begun producing a target-side effect, an executor
that remains alive and blocked forever never acks it. **The relay SHALL
therefore bound post-submission execution.** `[delivery].submission-timeout-
ms`, anchored at the point a packing unit's write begins, bounds it. When
that bound elapses without the covering `ack` having arrived, the relay
SHALL **initiate the generation fence**, and SHALL NOT terminalize the
member at that moment. Evidence continues to be accepted throughout the
bounded fence windows, and every still-unresolved member is terminalized
through the guard's evidence order at the verdict.

This is not a reintroduction of the timers this capability otherwise
forbids: a retired timer inferred *target failure* from absence; this bound
states a fact about the relay's own supervised wait for an `ack` — the
execution it is supervising overran, so it stops waiting and records that it
does not know. It is an execution watchdog and SHALL be described as one.

**Admission quota SHALL be released by the guard's terminal transition, and
by nothing else.** Releasing it anywhere else permits a double release on any
path that attempts termination twice.

#### Scenario: An unacknowledged write is bounded rather than waiting forever

- **WHEN** a packing unit's write has begun and no covering `ack` has arrived
  past `[delivery].submission-timeout-ms`
- **THEN** the generation fence is initiated
- **AND** no member is terminalized at that moment
- **AND** every still-unresolved member is terminalized through the guard's
  evidence order at the fence verdict

#### Scenario: Fence evidence still wins after the bound elapses

- **WHEN** the execution bound elapses and the fence's cooperative stop
  halts a bound member's unit before it produces any effect
- **AND** that unit records `NotSubmitted`
- **THEN** the member resolves `not_submitted` at the verdict
- **AND** it does not resolve `submission_unknown`
- **BECAUSE** the single resolution cut is the verdict, so evidence the
  fence produces is still admissible

#### Scenario: The execution bound does not override stronger evidence

- **WHEN** the execution bound elapses while one packing unit has already
  recorded `Submitted`
- **THEN** that unit's members resolve `delivered`
- **AND** only bound members lacking stronger evidence at the cut resolve
  `submission_unknown`

#### Scenario: The execution bound asserts nothing about the target

- **WHEN** the execution bound elapses
- **THEN** no member resolves to a failure spelling, and no target-health
  state is inferred
- **AND** a bound member lacking stronger evidence at the cut resolves
  `submission_unknown`
- **BECAUSE** the bound reports that the relay's own execution overran, not
  that the target is unhealthy

#### Scenario: Quota releases at terminalization, target barriers at the fence

- **WHEN** members are terminalized at the fence verdict
- **THEN** their admission quota and outcome-level barriers are released
- **AND** the target's mailbox visibility past a raw entry, and generation
  replacement, are released only on a **positive** verdict
- **AND** on a negative verdict other targets continue to progress while
  this one remains fail-stop

#### Scenario: Resolve exactly once under a delivery-loop panic

- **WHEN** the delivery-loop executor for a transport panics after writing
  but before calling `ack`
- **THEN** every member of the unit it wrote reaches exactly one terminal
  outcome
- **AND** each member's admission quota is released exactly once

#### Scenario: Duplicate terminalization converges

- **WHEN** two paths attempt to terminalize the same member (for example, a
  late duplicate `ack` and a fence-verdict resolution racing)
- **THEN** exactly one transition occurs
- **AND** the admission quota is released exactly once
- **AND** the losing attempt does not alter the recorded outcome

#### Scenario: An unbound member resolves not_submitted whatever the trigger

- **WHEN** the guard terminalizes a member that was never bound to a packing
  unit
- **AND** the trigger is a panic, a channel closure, a generation
  replacement, or graceful shutdown
- **THEN** the member resolves `not_submitted` in every case

#### Scenario: A recorded unit outcome outranks the lifecycle trigger

- **WHEN** a generation is replaced while one of its units has already
  recorded `Submitted`
- **THEN** that unit's members resolve `delivered`
- **AND** they are not downgraded to `submission_unknown` because a
  replacement occurred

#### Scenario: Resolve exactly once under an evidence-collector panic

- **WHEN** the relay's evidence collector panics after a unit's evidence is
  recorded but before the members it covers are terminalized
- **THEN** every member of that unit resolves from the recorded evidence
- **AND** no member is left without a terminal outcome

#### Scenario: A receipt is not refused for want of a reservation it holds none of

- **GIVEN** a member resolves to a non-delivered outcome and its sender has
  a live delivery-loop executor
- **WHEN** the relay admits the terminal-outcome receipt into the sender's
  mailbox, which holds no admission reservation
- **THEN** no admission check is attempted for it
- **AND** the receipt is peeked and written like any mailbox entry rather
  than refused as unauthorized

#### Scenario: An admitted member is still terminalized beside relay-originated work

- **WHEN** a transport peeks and writes a prefix mixing an admitted member
  and a relay-originated receipt in one packing unit
- **THEN** `ack` terminalizes the admitted member from its own evidence
- **AND** a failure to write the unit affects both members' evidence
  identically, since they share the same packing unit

#### Scenario: Never retry an acknowledged-uncertain entry

- **WHEN** a mailbox entry resolves `submission_unknown`
- **THEN** relay does not re-queue, re-peek, or duplicate it into the
  mailbox
- **AND** the sender receives a terminal-outcome receipt naming the
  uncertainty

### Requirement: In-Process Delivery Recovery Scope

The guarantees in this capability SHALL hold for a **surviving relay process
and graceful shutdown** only. This SHALL be stated as a limitation in
operator-facing documentation rather than implied.

An abrupt relay crash loses mailbox contents, submission evidence, outcomes,
and sender notification alike. No abstraction reconciles them after the
fact, and this capability does not claim to.

**Recovery behavior SHALL be specified only where it exists.** In-process
recovery is real and is specified: mailbox entries and cursors live in the
relay's own admission ledger, independent of any transport generation's
lifetime, so when a per-target transport is torn down and replaced within a
surviving relay process, the replacement generation's first `peek` sees
every entry the old generation had not yet acknowledged, in the same order.
An entry a prior generation wrote but had not yet acknowledged before
teardown MAY be re-served and re-written by the replacement generation — a
duplicate under at-least-once, not a lost message. Process-startup recovery
is **not** specified, because nothing persists across a process boundary.

On graceful shutdown, still-`queued` relay-owned members SHALL resolve
`dropped_on_shutdown`. A member whose packing unit has already begun
producing a target-side effect SHALL NOT resolve `dropped_on_shutdown`; it
resolves through the guard's evidence order.

**Shutdown budgets SHALL nest.** Graceful shutdown runs under one
process-wide **shutdown-work deadline**, and every bounded step on the
shutdown path SHALL size itself from what remains of it rather than from a
duration configured in isolation. A step SHALL reserve headroom for the
steps that follow it, and a step whose configured bound exceeds the
remaining budget SHALL be cut down to fit rather than allowed to overrun.

The shutdown-work deadline is **distinct from, and never later than, the
watchdog's forced exit**, established at the first of: the watchdog
observing the shutdown signal, or the first step to need a budget once
shutdown has been requested.

Consequently, a shutdown fence MAY be cut short by the deadline and return a
**negative verdict**. That verdict is a fail-safe: the process is exiting,
no replacement generation will be admitted, and unresolved members
terminalize through the guard's evidence order exactly as they would on any
other trigger.

**Resolving a member SHALL NOT depend on a step it does not require.**
Members that were never submitted to a transport SHALL resolve before the
shutdown fence runs, because nothing about their outcome depends on whether
a generation ceased.

#### Scenario: A shutdown fence cut short by the deadline still resolves every member

- **WHEN** the shutdown deadline leaves less time than the configured fence
  observation requires
- **THEN** the fence observes for the remaining budget rather than the
  configured duration
- **AND** a negative verdict resolves unresolved members through the
  evidence order
- **AND** no replacement generation is admitted, because the process is
  exiting

#### Scenario: Never-submitted members resolve before the fence

- **WHEN** relay shuts down gracefully with members still queued and never
  submitted to a transport
- **THEN** those members resolve `dropped_on_shutdown` before the generation
  fence begins
- **AND** their resolution does not depend on the fence's verdict or
  duration

#### Scenario: Re-serve an unacknowledged entry to a replacement generation

- **WHEN** a transport generation is torn down and replaced within a
  surviving relay process
- **THEN** every entry the old generation had not yet acknowledged is
  peekable by the replacement generation
- **AND** entries retain their position in the per-target mailbox order

#### Scenario: Never resubmit a member whose write had already begun

- **WHEN** a transport generation is replaced while one of its packing
  units has already begun producing a target-side effect
- **THEN** that unit's members resolve through the guard's evidence order
- **AND** they are not re-peeked or re-written by the replacement generation

#### Scenario: Separate queued and in-flight members at shutdown

- **WHEN** relay shuts down gracefully with a mix of `queued` members and
  members whose write has already begun
- **THEN** the `queued` members resolve `dropped_on_shutdown`
- **AND** the in-flight members resolve through the guard's evidence order,
  never `dropped_on_shutdown`
- **AND** an in-flight member never bound to a packing unit resolves
  `not_submitted` rather than `submission_unknown`

#### Scenario: State the crash-recovery limitation

- **WHEN** operator-facing documentation describes delivery guarantees
- **THEN** it states that they hold for a surviving relay process and
  graceful shutdown only
- **AND** it does not describe process-startup recovery behavior

## ADDED Requirements

### Requirement: Mailbox Peek Operation

The relay SHALL expose a read-only `peek(target, entry_max,
canonical_bytes_max)` operation to the transport owning `target`. `peek`
SHALL advance nothing: it does not authorize, does not reserve anything
beyond what admission already reserved, and MAY be called any number of
times without side effect on the mailbox.

`peek` SHALL return the head contiguous run of mail entries within
`entry_max` and `canonical_bytes_max`, stopping before any raw entry. **If
the head entry itself is a raw entry, `peek` SHALL return exactly that one
entry as a singleton** rather than an empty result, so a raw entry can never
park a mailbox permanently behind a bound too small to admit it alongside
mail.

Bounds SHALL be expressed only in units the relay can evaluate without
rendering: entry count and canonical payload bytes. Token budget SHALL NOT
be a `peek` bound; it is a property of an entry as a specific transport
would render it, which the relay does not know and SHALL NOT pretend to
know.

`peek` SHALL require the calling connection's bound consumer generation to
be the target's `active_generation_id`. A call from a superseded generation
SHALL be refused without returning any entries.

#### Scenario: Peek returns a contiguous mail prefix within bounds

- **WHEN** a transport calls `peek` with `entry_max` and
  `canonical_bytes_max` and the mailbox head is mail
- **THEN** the relay returns the longest contiguous prefix of mail entries
  that fits both bounds
- **AND** the mailbox is unchanged by the call

#### Scenario: A raw head entry is returned alone

- **WHEN** the mailbox head is a raw-kind entry
- **THEN** `peek` returns exactly that entry
- **AND** no mail entry is included even if it would fit the bounds

#### Scenario: Peek is safe to repeat

- **WHEN** a transport calls `peek` twice in a row without an intervening
  `ack`
- **THEN** both calls return the same entries
- **AND** neither call changes mailbox state

#### Scenario: A superseded generation cannot peek

- **WHEN** a `peek` call's consumer generation is not the target's
  `active_generation_id`
- **THEN** the relay refuses the call
- **AND** returns no entries

### Requirement: Mailbox Acknowledgment and Partial Acknowledgment

The relay SHALL expose `ack(target, generation_id, through_seq, evidence)`,
advancing the target's cursor and terminalizing every entry up to and
including `through_seq` from the supplied per-member `SubmissionEvidence`,
bound to `generation_id` as specified by `Consumer Generation Ownership and
Replacement`.

**Partial acknowledgment is the ordinary case, not an exception.** A
transport that peeked ten entries and wrote five MAY `ack` through the
fifth; the remaining five stay `queued`, in order, for the next `peek`.
No requirement SHALL treat a partial `ack` as needing a distinct code path
from a full one.

`ack` for an entry at or behind the current cursor SHALL be a no-op, per
`Mailbox Ordering and Cursor Lifecycle`'s duplicate-acknowledgment scenario.

#### Scenario: A partial acknowledgment leaves the remainder queued

- **WHEN** a transport peeks entries 1 through 10 for a target and writes
  only 1 through 5
- **AND** it calls `ack` with `through_seq = 5`
- **THEN** entries 1 through 5 terminalize per the guard's evidence order
- **AND** entries 6 through 10 remain `queued` and are included in the next
  `peek`

#### Scenario: A full acknowledgment terminalizes everything peeked

- **WHEN** a transport peeks and writes every entry it peeked
- **AND** it calls `ack` with `through_seq` covering all of them
- **THEN** every covered entry terminalizes per the guard's evidence order

### Requirement: Consumer Generation Ownership and Replacement

Exactly **one** consumer generation SHALL be active per target at a time.
`active_generation_id` is the only durable ownership datum the relay keeps
about mailbox consumption; it SHALL be checked on every `peek` and `ack`.

A **replacement** generation SHALL require a positive "old execution ceased"
guarantee before admission — never a heartbeat timeout, never elapsed time.
The guarantee SHALL be established by driving the existing `GenerationFence`
five-step mechanism (`transport-abstraction`) to a **positive** verdict for
the outgoing generation.

**A transport instance maintains exactly one serial delivery executor for
its lifetime, independent of any connection to it.** Same-generation
reconnect (the relay reattaching to a still-live transport instance) SHALL
NOT itself start a second delivery executor and SHALL NOT require revoking
the generation; it swaps the connection underneath the one executor already
running. A transport that spawns a delivery executor per inbound connection
does not satisfy this requirement.

**Socket or connection closure alone is not sufficient evidence of
cessation.** A closed connection observes cessation of that connection, not
of work the still-running executor already accepted from it. Only a
positive `GenerationFence` verdict, per the requirement above, establishes
that a generation has actually ceased.

#### Scenario: Exactly one generation is admitted per target

- **WHEN** a target has an active consumer generation
- **AND** a second connection attempts to bind as a consumer for the same
  target without going through replacement
- **THEN** the relay refuses the second binding

#### Scenario: Replacement requires a positive fence verdict

- **WHEN** a new generation attempts to replace the active one for a target
- **THEN** the relay drives the `GenerationFence` mechanism against the
  outgoing generation
- **AND** admits the replacement only on a positive verdict
- **AND** on a negative verdict leaves the target held, admitting no
  replacement

#### Scenario: Same-generation reconnect does not require a fence

- **WHEN** a transport instance's connection to the relay drops and it
  reconnects as the same generation
- **THEN** the relay attaches the existing connection state to the one
  still-running delivery executor
- **AND** does not invoke `GenerationFence` and does not admit a replacement

#### Scenario: A closed connection alone does not license replacement

- **WHEN** a target's consumer connection closes
- **AND** no positive `GenerationFence` verdict has been observed for that
  generation
- **THEN** the relay does not admit a replacement generation for that target

### Requirement: Revocation Serialized Against In-Flight Acknowledgment

Applying an `ack` and admitting a replacement generation SHALL be mutually
exclusive under the same lock the admission guard already uses for
terminalization (`Delivery Guard and Acknowledgment Terminalization`).

Processing an `ack` SHALL, as the first action inside that lock, compare the
`ack`'s supplied `generation_id` to the target's current
`active_generation_id`. A match SHALL allow the rest of `ack` processing to
proceed within the same critical section. A mismatch SHALL reject the `ack`
without applying any effect, leaving its named entries `queued` for
re-service to whichever generation is current.

Admitting a replacement generation SHALL, under the same lock, occur only
**after** a positive `GenerationFence` verdict for the outgoing generation
has been observed, per `Consumer Generation Ownership and Replacement`.

**These two rules together close the gap.** An `ack` that reaches the lock
while its generation is still active is either fully applied before any
replacement can be admitted (the lock excludes the replacement), or its
generation is already stale, in which case it is rejected rather than
applied. Neither path allows an `ack` bound to a superseded generation to
commit after the replacement is admitted.

#### Scenario: An in-flight ack commits before replacement is admitted

- **GIVEN** an `ack` call has already entered the critical section and its
  generation matches `active_generation_id`
- **WHEN** a replacement generation is concurrently being considered for the
  same target
- **THEN** the `ack` completes its effect before the replacement can acquire
  the lock to flip `active_generation_id`

#### Scenario: A stale ack is rejected rather than applied

- **GIVEN** a replacement generation has already been admitted for a target
- **WHEN** an `ack` bound to the superseded generation reaches the relay
- **THEN** the relay rejects it without advancing the cursor or releasing
  quota
- **AND** the entries it named remain `queued` for the current generation

### Requirement: Delivery Doorbell Notification

The relay SHALL notify a transport's delivery executor when its target's
mailbox transitions from empty to non-empty, or otherwise gains content
worth a `peek`. The notification SHALL carry **no data and no custody** — it
is a hint to call `peek`, not a substitute for calling it.

The doorbell SHALL be implemented as an injected closure the relay invokes,
following the existing upward-signal pattern already specified in
`transport-abstraction` (the readiness edge-hint a transport invokes today);
it is not a new signaling mechanism, only a new event it is invoked for.

**Losing a doorbell notification loses nothing but time.** A transport's
delivery executor SHALL also poll at a bounded cadence as a backstop, so a
missed notification only delays the next `peek` rather than losing or
misreporting a delivery. Correctness SHALL NOT depend on any doorbell
notification arriving.

The doorbell SHALL be rebuilt fresh per consumer generation at construction
time and SHALL NOT be persisted or reconstructed across a relay restart;
mailbox contents themselves, not the doorbell, are what a new generation's
first `peek` recovers.

#### Scenario: A doorbell notification prompts a peek

- **WHEN** a target's mailbox transitions from empty to non-empty
- **THEN** the relay invokes the target's doorbell closure
- **AND** the transport's delivery executor calls `peek` in response

#### Scenario: A missed doorbell only delays

- **WHEN** a doorbell notification is not observed by the transport's
  delivery executor
- **THEN** the executor's bounded poll still calls `peek` on its next tick
- **AND** no entry is lost or resolved without evidence because of the
  missed notification

### Requirement: Policy Admission Snapshot

Authorization policy governing whether an entry may be admitted SHALL be
evaluated **once, at admission**. The admission decision SHALL NOT be
re-evaluated at `peek` or `ack` time.

A policy change SHALL be **prospective only**: it governs entries admitted
after the change and has no effect on entries already sitting in a mailbox,
whether or not they have been peeked.

#### Scenario: An admitted entry survives a later policy tightening

- **GIVEN** an entry was admitted under a policy that permitted it
- **WHEN** the bundle's authorization policy changes to forbid that class of
  entry
- **THEN** the already-admitted entry remains `queued` and deliverable
- **AND** it is not purged, blocked, or re-authorized on account of the
  policy change

#### Scenario: A policy change governs only future admissions

- **WHEN** a bundle's authorization policy changes
- **THEN** a new send request is evaluated against the policy in effect at
  the time of that request
- **AND** no previously admitted mailbox entry is re-evaluated

### Requirement: Mailbox Retention and Quota Bounds

The mailbox SHALL remain in-memory and non-durable, as specified by
`Mailbox Ordering and Cursor Lifecycle`. No durable persistence format for
mailbox contents SHALL be introduced by this capability.

Mailbox growth for a target with unacknowledged entries SHALL be bounded
exclusively by the existing per-target and relay-global admission quota
(envelope count and canonical payload bytes). No separate time-to-live or
pruning mechanism SHALL be introduced to bound queued-entry growth.

A target's mailbox and cursor state SHALL be eligible for cleanup when its
worker-registry entry is reaped on generation teardown without a
replacement being admitted — an existing lifecycle event, not a new one
introduced by this requirement.

#### Scenario: Quota, not a TTL, bounds an unconsumed mailbox

- **WHEN** a target accumulates entries without peeking or acknowledging
  them
- **THEN** growth stops at the existing per-target admission quota
- **AND** no entry is dropped by elapsed time alone

#### Scenario: An empty mailbox is cleaned up with its worker registration

- **WHEN** a target's mailbox has no queued entries
- **AND** its worker-registry entry is reaped because no replacement
  generation was admitted
- **THEN** the target's mailbox and cursor state are eligible for cleanup at
  the same time
