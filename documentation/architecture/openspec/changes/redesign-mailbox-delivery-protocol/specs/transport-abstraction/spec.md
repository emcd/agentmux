## MODIFIED Requirements

### Requirement: Transport Interface Contract

The relay delivery subsystem SHALL NOT dispatch delivery by invoking a
transport method per envelope. Instead, each transport implementation of the
`Transport` trait SHALL own one
**serial delivery-loop executor**, spawned during `startup` and living for the
transport instance's lifetime, which calls the relay's `peek`, `declare`,
and `ack` entry points (`delivery-quiescence`'s `Mailbox Peek Operation`,
`Mailbox Submission Declaration`, and `Mailbox Acknowledgment and Partial
Acknowledgment` requirements) directly.

`mailw`, `raww`, and `is_ready_for_handover` are **removed from the `Transport`
trait**. The relay no longer invokes a transport to deliver mail, and no
longer reads a readiness level from it to gate anything. A transport MAY
still keep an internal readiness predicate — the `transport-contracts`
capability's prompt-readiness templates still exist and still govern
Tmux/Pty readiness determination — but that predicate now informs only the
transport's own choice of when to write what it peeked, not any relay-facing
call.

The delivery-loop executor's contract:

- it calls `peek(target, entry_max, canonical_bytes_max)` when notified by
  the delivery doorbell or when its bounded poll fires, per
  `delivery-quiescence`'s `Delivery Doorbell Notification` requirement;
- it decides what it will attempt to write: rendering what it peeked using
  its own transport-specific representation, measuring the result against
  its own token budget, and MAY coalesce consecutively peeked mail entries
  into one packing unit exactly as `mailw` invocations were previously
  coalesced;
- it calls `declare(target, generation_id, through_seq)` for exactly that
  decided prefix — possibly all of what it peeked — **before** attempting
  to write any of it, per `delivery-quiescence`'s `Mailbox Submission
  Declaration` requirement;
- only then does it attempt the write;
- it calls `ack(target, generation_id, packing_unit_id, evidence)` naming
  the `PackingUnitId` `declare` returned, supplying per-member
  `SubmissionEvidence` derived from the write attempt — `Submitted` on
  success, `NotSubmitted` if it can positively establish nothing happened,
  `SubmissionUnknown` otherwise;
- it does not wait on prompt readiness, target turn completion, target
  output, or an operator decision before writing once it has decided to
  write; a submission primitive that can block SHALL be supervised and
  fenced/interruptible per the `Transport Generation Fencing and
  Termination Authority` requirement, unchanged by this proposal.

Coalescing remains permitted for the same reason it was permitted under the
push model: the partition is declared before any target-side effect via
`declare`, so the group is recorded even though its membership is
timing-derived. `declare` is the pull-model's relocation of what
`PartitionSink` did under the push model — the same pre-effect binding
discipline, called from the transport's own delivery loop instead of from
a relay-invoked submission path.

**The write path remains fallible**, and a refusal remains a terminal
evidence result rather than a reclaim: a transport that decides, after
peeking, not to write anything simply does not call `declare` for those
entries, leaving them `queued` and undeclared for the next attempt, which
MAY be made by the same generation or a replacement. A transport that has
already declared but then fails to write MUST still resolve that
declaration — by acking with `NotSubmitted` or `SubmissionUnknown` evidence
as appropriate — rather than leaving it to time out against
`[delivery].submission-timeout-ms`, though the watchdog remains the
backstop if the executor cannot do even that (for example because it has
panicked).

Each transport type SHALL implement its delivery-loop executor in its own
module. `TransportImpl` retains its five variants — `Acp`, `Tmux`, `Pty`,
`Ui`, and `Pubsub` — and this contract applies to all of them.

The legacy synchronous methods — `deliver`, `prepare_delivery`, and
`raw_write` — remain not retained, as under the prior contract.

`raww` as a **relay-inbound** operation name is unaffected by this
requirement: a caller still invokes `raww` to submit raw input, and the
relay still admits it, but as a raw-kind mailbox entry rather than as a
direct push into a transport. `transport-contracts`' `Relay raww transport
behavior` requirement specifies how a transport's delivery-loop executor
discovers and writes it.

#### Scenario: ACP delivery via its own delivery-loop executor

- **WHEN** an ACP target's mailbox gains entries
- **THEN** the ACP transport's delivery-loop executor peeks them, having
  coalesced consecutive entries or not, declares the resulting packing
  unit, renders pane-envelope text internally, submits each unit as its own
  `session/prompt` request, and acks what it wrote
- **AND** it does not park an unwritten peek behind an in-flight turn

#### Scenario: Tmux delivery via its own delivery-loop executor

- **WHEN** a Tmux target's mailbox gains entries
- **THEN** the Tmux transport's delivery-loop executor peeks them, having
  coalesced consecutive entries or not, into token-budget prompts, declares
  each resulting packing unit, injects each separately, and acks what it
  wrote
- **AND** it does not wait for pane quiescence beyond its own readiness
  check before writing

#### Scenario: UI delivery via its own delivery-loop executor

- **WHEN** a `Ui` target's mailbox gains entries
- **THEN** the UI transport's delivery-loop executor peeks them, declares
  the packing unit, and emits the messages as relay stream events through
  its injected broadcaster closure, then acks what it emitted
- **AND** no `Ui`/`Pubsub` delivery short-circuit appears in the mailbox
  path

#### Scenario: A peeked-but-undeclared entry leaves the mailbox untouched

- **WHEN** a transport's delivery-loop executor peeks entries but decides
  not to write them — because its own readiness check fails, or its write
  channel is full or closed
- **THEN** it does not call `declare` for those entries
- **AND** they remain `queued` and undeclared for the next `peek`, by the
  same generation or a replacement
- **AND** the relay does not treat the un-declared peek as a refusal
  requiring its own terminal outcome

#### Scenario: A declared-but-failed write resolves through acknowledgment

- **WHEN** a transport's delivery-loop executor has declared a packing unit
  and its write attempt then fails in a way it can observe (the write
  channel closes, or the target refuses it)
- **THEN** the executor calls `ack` for that unit with `NotSubmitted` or
  `SubmissionUnknown` evidence, whichever it can positively establish
- **AND** it does not leave the declaration to expire against
  `[delivery].submission-timeout-ms` when it is still able to report

#### Scenario: Shutdown resolves undeclared members

- **WHEN** relay shutdown is requested
- **THEN** still-`queued` relay-owned members that no delivery-loop executor
  has declared resolve `dropped_on_shutdown`
- **AND** declared members resolve from evidence per the
  `delivery-quiescence` capability's guard

#### Scenario: Startup never runs on an async runtime thread

- **WHEN** the relay invokes `Transport::startup` for any session type
- **THEN** it runs the call on a blocking thread rather than on a runtime
  worker thread, because `startup` is synchronous on the trait and every
  implementation of it is therefore permitted to block, including spawning
  its own delivery-loop executor
- **AND** the relay SHALL NOT make that choice per session type
- **AND** because such a call cannot be aborted, each transport's `startup`
  SHALL own the cleanup of anything it created, reaching its own
  conclusion even when the caller awaiting it has gone away

### Requirement: Transport Handover Capacity and Readiness

The system SHALL recognize two relay-facing quantities, where three were
previously conflated:

| Quantity | Owner | Purpose |
|---|---|---|
| **Admission quota** | relay | how much may be queued per target and relay-global; enforced at admission |
| **Maximum peek dimensions** | transport, static | the most a `peek` call may return at once |

**Acceptance capacity is no longer a relay-facing quantity.** Under the push
model a transport surfaced "can I accept right now" as `is_ready_for_
handover`, which the relay read to decide whether to authorize. Under the
pull model there is nothing to authorize: a transport that cannot currently
write simply does not peek, or peeks and writes nothing. Whether a transport
is momentarily busy is therefore entirely its own internal state, consulted
by its own delivery-loop executor, and is not part of this contract.

All relay-facing quantities SHALL be expressed in units the relay can
evaluate without packing: **envelope count and canonical payload bytes**,
where canonical bytes means the serialized envelope payload the relay
already holds, not rendered target text. Declaring them in tokens would be
circular, since only the transport can render and count those.

**No transport → relay back-edge for readiness.** The relay does not read a
readiness level from a transport at all under this contract; readiness is
purely internal to the transport's own delivery-loop executor. Where a
transport needs to prompt the relay that its target's own state changed in a
way worth reporting (for example, to drive `look` freshness), it SHALL do so
through an opaque closure the relay provided at construction, unrelated to
mailbox consumption.

#### Scenario: Maximum peek dimensions are declared in relay-evaluable units

- **WHEN** a transport declares its maximum peek dimensions
- **THEN** they are expressed in envelope count and canonical payload bytes
- **AND** not in rendered tokens

#### Scenario: A busy transport is not a relay-visible state

- **WHEN** a transport's delivery-loop executor is not ready to write
- **THEN** it does not call `peek`, or calls `peek` and writes nothing
- **AND** the relay observes no readiness signal from the transport and
  makes no decision on the basis of one

### Requirement: Transport Module Boundaries

ACP-specific delivery code SHALL reside in `src/acp/`. Tmux-specific delivery
code SHALL reside in `src/tmux/`. Pty-specific delivery code SHALL reside in
`src/pty/`. UI stream-broadcast delivery code SHALL reside in its own transport
module (`UiTransport`), not in the relay delivery subsystem.

The boundary SHALL distinguish three concerns that were previously four:

| Concern | Owner |
|---|---|
| **Mailbox custody** — what is queued for a target, in what order, and for how long | **relay** |
| **Consumption timing** — when to peek, declare, write, and ack | **transport** |
| **Rendering and packing** — target representation and partition into packing units | **transport** |

**"Readiness scheduling" is retired as a distinct relay-owned concern, not
relocated.** Under the push model, the relay decided *when* to authorize a
target's next handover, informed by a readiness level the transport
reported. Under the pull model there is no relay-side scheduling decision
left to make: the relay holds custody and answers `peek`/`declare`/`ack`;
the transport decides entirely on its own when those calls are worth
making. What survives from the old "readiness determination" concern is
unchanged — a prompt regex over a pane tail is meaningless for ACP, whose
readiness is an earlier turn completing on the wire protocol with no
snapshot to inspect, and meaningless again for UI, whose readiness is
subscriber connectivity — but it is now folded entirely into "consumption
timing," transport-owned end to end, because there is no relay-side
counterpart to split it against.

**The relay reads no readiness level from any transport.**
`is_ready_for_handover` does not exist on the `Transport` trait; there is
nothing for the relay to learn "as a level" any more. Only the transport
can render target text and count its tokens, so `prompt_tokens_max` remains
an internal packing-unit limit invisible to the relay, exactly as before.

The relay delivery subsystem SHALL NOT contain transport-specific logic; all
transport dispatch SHALL go through `TransportImpl`. Specifically, the relay
delivery subsystem SHALL NOT contain:

- batch-combining or prompt-packing logic,
- pane-envelope rendering for coder transports,
- prompt-readiness evaluation: no `prompt_regex` compilation or matching, no pane
  output inspection, and no cursor-column comparison,
- per-transport `TargetConfiguration` dispatch arms for delivery, nor a
  relay-internal UI delivery path.

Every target SHALL be transport-delivered. The only target-type-dependent step is
transport construction.

#### Scenario: ACP code in src/acp/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific types or functions are present

#### Scenario: Tmux code in src/tmux/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Tmux pane operations, rendering, or session lifecycle primitives are
  present

#### Scenario: Pty code in src/pty/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Pty transport operations, libghostty-vt state access, or
  portable-pty I/O are present

#### Scenario: Packing stays with the transport

- **WHEN** a developer looks for the logic that splits received envelopes into
  packing units
- **THEN** they find it in the owning transport module
- **AND** the relay expresses its own limits only in envelope count and canonical
  payload bytes, never in rendered tokens

#### Scenario: UI target delivered through its transport, not a relay path

- **WHEN** the relay's mailbox holds entries for a `Ui` target
- **THEN** its delivery-loop executor is dispatched through `TransportImpl::Ui`
  uniformly, with no transport-type routing fork
- **AND** no `TargetConfiguration::Ui | Pubsub` delivery arm or UI delivery
  short-circuit appears anywhere in the relay

#### Scenario: Mailbox custody lives relay-side

- **WHEN** a developer looks for the queued entries for a target
- **THEN** they find one relay-owned mailbox rather than a per-transport buffer
- **AND** no transport retains envelopes awaiting its own readiness condition
  outside that mailbox

#### Scenario: Consumption timing stays with the transport

- **WHEN** a developer looks for the logic that decides when a target's
  delivery-loop executor peeks, declares, and writes
- **THEN** they find it entirely in the owning transport module
- **AND** `src/relay/delivery/` contains no prompt-regex matching, pane
  inspection, or cursor-column comparison
- **AND** the relay reads no readiness level from the transport at all

### Requirement: Synchronous Delivery Completion

Each member of a declared packing unit SHALL resolve with a terminal
`SingleDeliveryOutcome`; the relay worker maps that outcome onto its `SendResult`
(the outcome carries the transport-side type, not the relay `SendResult`,
preserving the no-relay-dependency invariant).

**Outcomes are per packing unit, not per batch.** If one unit submits and another
fails, their members SHALL receive different outcomes. A transport SHALL NOT
apply one outcome to every member of a declared unit's neighbors.

Every member's outcome SHALL be **derived from its unit's immutable evidence
record**, never from live re-inspection at fan-out time.

The transport SHALL NOT drop a declared member without resolving it (via `ack`),
and the relay-owned guard SHALL terminalize any declared member the transport
fails to resolve, selecting the outcome by the guard resolution order defined in
the `delivery-quiescence` capability's `Delivery Guard and Acknowledgment
Terminalization` requirement. This does not block the relay request path: the
send RPC returns `queued` at admission, well before any declaration exists.

#### Scenario: Member outcome resolves through the relay worker

- **WHEN** a transport's delivery-loop executor acks a declared packing unit
- **THEN** each member resolves with a terminal `SingleDeliveryOutcome`
- **AND** the relay worker maps that outcome onto its `SendResult` at the collect
  site, without the transport referencing any `crate::relay` type

#### Scenario: Differing outcomes across packing units

- **WHEN** one packing unit is acked `Submitted` and another `NotSubmitted`
- **THEN** unit 1's members resolve `delivered`
- **AND** unit 2's members resolve `not_submitted`
- **AND** neither result is applied to the other unit's members

#### Scenario: An earlier unit's success is not retracted

- **WHEN** a transport's delivery-loop executor fails or panics while
  writing a later declared unit
- **THEN** the members of already-acked units keep their `delivered` outcome

#### Scenario: The guard resolves what the transport does not

- **WHEN** a transport's delivery-loop executor exits without acking some
  declared members
- **THEN** the relay-owned guard terminalizes them by its evidence order — the
  unit's record if one exists, `submission_unknown` if declared with no
  evidence
- **AND** each member's admission quota is released exactly once

### Requirement: Worker Readiness Interface

The relay SHALL expose worker readiness through a transport-agnostic interface,
not an ACP-specific one. Any worker-driven transport that maintains a multi-state
readiness lifecycle SHALL populate the same surface:

- a transport-neutral readiness enum `WorkerReadinessState` with variants
  `Initializing`, `Available`, `Busy`, `Recovering`, and `Unavailable`, carrying
  no ACP-specific naming;
- a per-target registry field holding one `Option<WorkerReadinessState>`;
- relay-internal mutator/reader `set_worker_readiness` / `get_worker_readiness`;
- an in-process observer `subscribe_worker_readiness` (with publisher
  `publish_worker_readiness`) that yields the current readiness and every
  subsequent transition, and that MAY be subscribed before the worker registers
  and continues to receive transitions after the worker unregisters;
- a public read `read_worker_readiness` returning `Option<&'static str>`.

**This interface's consumers changed with the pull model; the interface
itself did not.** It previously backed both `is_ready_for_handover`'s ACP
implementation and `look`-freshness/`OutputView` prime-wait. The first
consumer no longer exists: `is_ready_for_handover` is not on the `Transport`
trait, so nothing relay-facing reads this state to gate delivery. Its
remaining consumers are the owning transport's own delivery-loop executor
(deciding internally whether to declare and write, exactly where prompt-
readiness determination already lived) and `look`-freshness/prime-wait,
unchanged. The mutator/observer/reader surface itself is unaffected — what
changed is that no external, relay-facing gate reads it any more.

A transport SHALL NOT latch readiness to `Unavailable` on the basis of an
absence-derived delivery failure, because no such failure exists under this
contract. Readiness transitions SHALL be driven by positively observed lifecycle
events.

The interface SHALL NOT spell any of these symbols with an `acp`/`Acp` prefix.
Transport-specific readiness *triggers* SHALL remain in the owning transport
module, which drives the shared interface rather than defining its own readiness
type.

#### Scenario: ACP worker populates the shared readiness interface

- **WHEN** the ACP worker transitions readiness (e.g. to `busy` on prompt-write
  success)
- **THEN** it calls `set_worker_readiness` with a `WorkerReadinessState` value
- **AND** in-process subscribers observe the transition
- **AND** `read_worker_readiness` returns the corresponding state string

#### Scenario: Pty worker populates the shared readiness interface

- **WHEN** the Pty worker transitions readiness on a positively observed
  lifecycle event, such as child exit
- **THEN** it calls `set_worker_readiness` with a `WorkerReadinessState` value
- **AND** in-process subscribers observe the transition

#### Scenario: Readiness surface carries no ACP-specific naming

- **WHEN** a developer reads the worker readiness enum, registry field, observer,
  and public read
- **THEN** none is spelled with an `acp`/`Acp` prefix

#### Scenario: Subscription survives the worker registration window

- **WHEN** a caller subscribes before the worker for that target is registered
- **THEN** the subscription is established against the per-target publisher
- **AND** the caller observes transitions published once the worker exists and
  after it later unregisters

#### Scenario: Readiness is not latched from an inferred failure

- **WHEN** a delivery resolves `submission_unknown`
- **THEN** the transport does not latch readiness to `Unavailable` on that basis
- **AND** readiness continues to reflect positively observed lifecycle state

#### Scenario: No relay-facing consumer reads worker readiness

- **WHEN** a developer searches the relay delivery subsystem for a reader of
  `WorkerReadinessState`
- **THEN** they find none — its only consumers are the owning transport's own
  delivery-loop executor and the `look`-freshness/prime-wait path

### Requirement: Positive Activity Signal

Each transport whose target produces observable output SHALL expose a
cross-transport activity signal from a transport-native
**terminal-output-write** primitive. The signal is a monotonic `u64` generation
that advances when bytes flow to the target's terminal, independently of whether
captured content visibly changed.

**The transport's own delivery-loop executor consumes this signal**; no
transport classifies on it as a delivery failure. An advance between two
consecutive observations SHALL be treated as a positive indication that the
target is active, and SHALL cause the executor to defer its own decision to
write for that iteration — leaving any peeked-but-undeclared entries queued.
This is entirely transport-internal now: there is no relay-side handover
decision left for the signal to suppress.

**Scope (terminal-output-write, not process-busy):** the field carries a marker
of bytes being written to the target's terminal. Its **absence SHALL NOT be
treated as a signal of any kind.** A target that is quiet may be hung, may be
awaiting an operator, or may be working silently, and nothing distinguishes them.

A transport that does not track activity, or whose primitive is unavailable,
SHALL populate the field with the constant `0`. A constantly-`0` signal can never
advance, so such a target's delivery-loop executor is never deferred on this
basis.

**Tmux is the only transport required to track one.** ACP has no terminal to
write to, and Pty — whose terminal writes would supply an obvious primitive —
remains work-in-progress for this release, so it reports the constant. This is
the fallback above being used as designed rather than a gap: the requirement is
that a transport either supplies a real marker or supplies one that can never
advance, and both are conformant.

#### Scenario: Tmux probe populates the activity signal from window_activity

- **WHEN** the Tmux probe observes and `#{window_activity}` returns a non-empty
  value
- **THEN** the resulting activity generation is the parsed `u64` epoch-seconds
  value of that marker

#### Scenario: Tmux probe falls back to 0 when window_activity is unavailable

- **WHEN** the Tmux probe observes and `#{window_activity}` is unavailable on the
  running tmux version
- **THEN** the resulting activity generation is `0`
- **AND** no advance is possible, so its delivery-loop executor is never
  deferred on this basis for that target

#### Scenario: Activity advance defers the executor's own write decision

- **WHEN** a target's activity generation advances between two consecutive
  observations by its own transport's delivery-loop executor
- **THEN** the executor does not declare or write for that iteration
- **AND** any peeked entries remain `queued` and undeclared

#### Scenario: Absence of activity produces no outcome

- **WHEN** a target's activity generation does not advance across any number of
  observations
- **THEN** no terminal outcome is produced on that basis
- **AND** its entries remain `queued`, resolving only if later declared and
  acked, if the transport is positively observed torn down, if it is
  continuously observed `Unreachable` past `[delivery].unreachable-dwell-ms`,
  or at relay shutdown
- **BECAUSE** the absence of an activity advance is not evidence; sustained
  unreachability is, which is why the dwell resolves an entry and a quiet screen
  never does

### Requirement: Pty Transport Implementation

The system SHALL provide a `PtyTransport` that implements the
`Transport` trait and is wired into `TransportImpl::Pty`. The
transport SHALL own one `libghostty_vt::Terminal<'static, 'static>`, one
`portable_pty` master, one reader thread, and one delivery-loop executor.
Because all `libghostty_vt` types are `!Send + !Sync`, the terminal SHALL live
on the delivery-loop thread and be reached from other threads through a
`SnapshotRequest` channel.

**Pty SHALL declare, then write** — the ordering Tmux already uses. The
transport SHALL NOT write any member to the PTY master before that member's
declaration is recorded via `declare`. Writing before declaration is what
made a flush group's membership mutable after its write, which is the defect
behind `agentmux:issues/relay/62`; declaration is the pull model's relocation
of the same discipline, at the same point in the sequence.

**Pty members SHALL be singleton packing units** unless a future change genuinely
combines them into one write, because the transport writes each member with its
own `write_all` pair. Each unit's outcome SHALL be derived from its own
evidence, and one outcome SHALL NOT be applied to every member of a group.

No envelope SHALL be absorbed into a packing unit after that unit is declared.

#### Scenario: Pty startup spawns the child PTY and installs effect handlers

- **WHEN** the relay calls `TransportImpl::Pty(t).startup(context)` for a
  Pty-backed bundle member
- **THEN** the transport opens a `portable_pty` master sized to the per-coder
  `cols` and `rows`
- **AND** spawns the configured child command with `COLORTERM=truecolor` and a
  `TERM` env-var value derived from the per-coder `term-protocol` field
- **AND** constructs a `libghostty_vt::Terminal` with the same dimensions and
  installs the canonical effect handlers
- **AND** spawns the reader thread and the delivery-loop executor
- **AND** the worker thread publishes `WorkerReadinessState::Available` AFTER
  successful `Terminal::new` + handler installation

#### Scenario: Pty startup does not wait for the worker to initialize

- **WHEN** `startup` has spawned the child, the worker thread, and the reader
  thread
- **THEN** it returns `TransportReadiness::Pending` immediately rather than
  waiting for the worker to report that it initialized
- **AND** the worker publishes `WorkerReadinessState::Available` when it has
  genuinely initialized; this gates only the delivery-loop executor's own
  decision to declare and write, and the `startup` return value carries no
  readiness answer
- **AND** a worker that never arrives is treated as a target that is never
  ready — its entries stay `queued` and undeclared, bounded in consequence by
  per-target admission quota rather than by a clock, exactly as for every
  other transport

**Reason:** the wait could not be made safe in either direction. Unbounded it
never reached a verdict. Bounded, its cleanup joined the worker thread, so a
startup that timed out *because that thread had stalled* then blocked on the
same stall — the bound relocated the hang rather than ending it. An in-process
`Terminal::new` cannot be interrupted, so no cleanup can promise a cessation it
has not observed, and a bound that cannot be made true is worse than none.

#### Scenario: Pty initialization failure becomes a reachability signal

- **WHEN** the Pty worker thread fails to construct its terminal, after
  `startup` has already returned
- **THEN** it publishes `WorkerReadinessState::Unavailable` and terminates its
  child, so the reader observes EOF and the transport reports `Unreachable`
- **AND** the member resolves through the unreachable dwell rather than waiting
  on a runtime that is never coming, which is the treatment reserved for a
  target that might still arrive

#### Scenario: A teardown during initialization is not overwritten

- **WHEN** shutdown is requested while the Pty worker is still initializing
- **THEN** the worker SHALL NOT publish `Available` for a runtime already being
  torn down
- **AND** the worker SHALL check the shutdown flag at every point it controls,
  leaving as the unobserved window only what precedes its first check: the
  uninterruptible terminal construction and the handler installation beside it

#### Scenario: Pty declares and writes a peeked entry immediately

- **WHEN** the Pty transport's delivery-loop executor peeks an entry it is
  ready to write
- **THEN** it declares each received envelope as its own singleton unit before
  writing any bytes
- **AND** writes each unit to the PTY master without waiting for quiescence
  beyond its own readiness check
- **AND** acks each member from its own unit's evidence

#### Scenario: No envelope is absorbed into a declared packing unit

- **WHEN** a new envelope is admitted while a packing unit for the same
  target is declared and outstanding
- **THEN** it forms part of a later declaration
- **AND** it is not added to the already-declared unit
- **BECAUSE** a mutable unit membership is what allowed one outcome to be
  reported for members that were written and members that were not

#### Scenario: Pty look renders formatter text + cursor via snapshot channel

- **WHEN** the relay calls `OutputView::look` on the `PtyOutputView` returned by
  `give_output`
- **THEN** the look implementation sends a `SnapshotRequest` through the
  `snapshot_tx` channel and returns
  `LookSnapshotPayload::Lines { snapshot_lines }`

#### Scenario: Pty shutdown kills the child before joining transport threads

- **WHEN** the relay calls `TransportImpl::Pty(t).shutdown()`
- **THEN** the transport publishes `WorkerReadinessState::Unavailable`
- **AND** sets `shutdown_flag = true`
- **AND** calls `child.kill()` followed by `child.wait()` BEFORE joining the
  reader thread or the worker thread
- **AND** joins the reader thread handle and the delivery-loop executor handle

### Requirement: Transport Generation Fencing and Termination Authority

A transport generation SHALL be **torn down and fenced before its replacement
begins**, so an old generation cannot submit after its `declared` entries were
resolved against it. Without fencing, "resolved unknown" and "still able to act"
coexist, which is a target-side ordering hazard.

**Marking a generation fenced is not a fence.** A submission already past its
check will still produce its effect, so a generation that is marked but not
acknowledged can write to the target after its members have been resolved. Only a
positive fence acknowledgment establishes that execution has ceased.

**Fence acknowledgment SHALL follow an explicit five-step state machine.** Each
step is distinct, and no step blocks:

1. **Cooperative stop request** — mark the generation fenced. An executor that
   checks the flag stops at its next check. This step is a signal, not a wait.
2. **First bounded cessation observation** — observe for up to
   `[delivery].fence-observation-timeout-ms` whether every generation-owned executor has
   ceased. If all have, go to step 5 positive.
3. **Forced generation termination** — invoke the transport's generation
   termination primitive. **The invocation SHALL be non-blocking**: it initiates
   termination and returns, consuming none of the acknowledgment budget. Waiting
   for its effect belongs to step 4, not to the call.
4. **Second bounded cessation observation** — observe for a further
   `[delivery].fence-observation-timeout-ms`.
5. **Verdict** — **positive** if every generation-owned executor has been
   observed to cease; **negative** otherwise. Timeout and failure both route to
   negative; there is no third outcome.

Total acknowledgment is therefore bounded by **twice**
`fence-observation-timeout-ms`, because steps 1 and 3 are non-blocking and only steps 2
and 4 consume time.

**Neither observation SHALL be a blocking join.** No runtime primitive can force
a thread blocked in a syscall to return, so a blocking join would reintroduce the
unbounded wait the bound exists to close. The supervisor observes cessation and
gives up on its own clock.

Steps 1 and 3 are genuinely different actions, and conflating them was a defect
in an earlier draft. Step 1 asks an executor that is *able* to observe a flag to
stop, and costs nothing when it works. Step 3 is the hard action for an executor
that cannot observe anything, and it is destructive — it terminates a child or
drops a broadcaster. Escalating straight to step 3 would destroy a child that was
about to stop cooperatively.

A generation supervisor SHALL retain the termination primitive **plus every
submission and permission executor handle it owns**. An executor whose handle is
discarded cannot be observed and cannot be fenced.

**The escalation action is fixed, not configurable**, because only one class of
action can unblock an executor that observes nothing. Step 3 SHALL invoke the
**generation termination primitive** that the transport declares.

Every transport SHALL declare such a primitive. Its contract is to **initiate
cessation of every effect path the generation owns, and to return without
blocking.** It is not "kill the child" — that is one implementation, and it does
not generalise to a transport owning no child process or reaching its target
through a process it does not own:

| Transport | Step 3 initiates | Step 4 observes |
|---|---|---|
| ACP, Pty | signal the generation's child to terminate, closing the stdin pipe or pty master being written to | the child reaped and the executor returned |
| Tmux | signal the generation's owned `tmux` client invocations to terminate. The tmux **server** is not owned by the generation and SHALL NOT be terminated — doing so would destroy the operator's sessions | those invocations exited and the executor returned |
| UI | drop the generation's broadcaster handle and subscriber senders | no further frame emitted and the executor returned |

**A successful primitive invocation does not acknowledge the fence.** Only
*observed cessation* does. The primitive initiates; step 4 observes. Reaping a
child and confirming an executor returned are observations, and they belong to
step 4 where they are bounded — putting them inside step 3 would place unbounded
waiting inside a call the bound does not cover.

The primitive is what makes escalation effective rather than what makes it sound:
it unblocks an executor blocked writing into the terminated path, so that
step 4's observation can succeed where step 2's could not.

When the verdict is **negative**, the supervisor SHALL NOT admit a replacement
generation for that target, SHALL NOT release its raw barrier, and SHALL record
the condition as observability.

This is deliberately fail-stop: a target that stops accepting new generations is
recoverable by operator action, and a target whose old generation might still
write while a new one runs is not.

A negative fence SHALL NOT hold any member's outcome open. Members resolve
through the guard's evidence order regardless, so an unceasing executor stalls
that target's lifecycle without stranding a single message.

Three facts SHALL be kept separate:

| Fact | Established by | Releases |
|---|---|---|
| **Outcome terminal** | guard transition to a terminal spelling | admission quota, receipts, outcome-level barriers |
| **Execution ceased** | fence acknowledgment | nothing on its own |
| **Target-side ordering safe** | execution ceased **and** no in-flight primitive can still take effect | the raw barrier |

`submission_unknown` is **terminal**. It resolves the member, releases quota, and
releases outcome-level barriers immediately. It does **not** establish that
execution has stopped. `submission_unknown` MAY therefore terminalize before the
fence is positive; **replacement and normal ordering barriers SHALL NOT proceed**
until it is.

A submission stopped by the fence before producing its effect SHALL resolve
`not_submitted`, since the fence is positive evidence that nothing was written.

#### Scenario: A positive fence stops the old generation

- **WHEN** a generation's fence reaches a positive verdict
- **THEN** no member of that generation is submitted afterwards
- **AND** replacement and the raw barrier are released

#### Scenario: A marked but unobserved generation is not fenced

- **WHEN** a generation is marked fenced and its members are resolved without
  cessation having been observed
- **THEN** an in-flight submission may still produce a target-side effect
- **AND** this SHALL NOT be treated as a fenced generation

#### Scenario: A successful primitive invocation is not an acknowledgment

- **WHEN** the generation termination primitive returns successfully
- **AND** cessation has not yet been observed
- **THEN** the fence is not yet positive
- **AND** replacement and the raw barrier remain held until step 4 observes
  cessation

#### Scenario: A discarded executor handle cannot be fenced

- **WHEN** a transport spawns a submission or permission executor without
  retaining its handle
- **THEN** the generation supervisor cannot observe its cessation
- **AND** the transport does not satisfy this requirement

#### Scenario: A cooperative stop is tried before forced termination

- **WHEN** a generation is fenced and its executors observe the fenced flag
- **THEN** they cease within the first bounded observation window
- **AND** the termination primitive is never invoked
- **BECAUSE** forced termination is destructive, and escalating straight to it
  would terminate a child that was about to stop on its own

#### Scenario: The first observation is bounded and escalates

- **WHEN** `[delivery].fence-observation-timeout-ms` elapses without every
  generation-owned executor having ceased
- **THEN** the supervisor invokes the transport's generation termination
  primitive
- **AND** that invocation returns without blocking
- **AND** a second bounded observation window of the same duration begins

#### Scenario: Escalation succeeds within the post-escalation window

- **WHEN** the termination primitive has been invoked
- **AND** every generation-owned executor is observed to cease within the second
  bounded window
- **THEN** the fence becomes positive
- **AND** replacement and the raw barrier are released

#### Scenario: Escalation not observed to complete leaves the fence negative

- **WHEN** the termination primitive has been invoked
- **AND** at least one generation-owned executor has not been observed to cease
  when the second bounded window elapses
- **THEN** the fence remains negative
- **AND** no replacement generation is admitted for that target and its raw
  barrier is not released
- **AND** the condition is recorded as observability
- **BECAUSE** timeout and failure both route here; a supervisor that cannot
  positively establish cessation SHALL NOT assume it

#### Scenario: A transport without an owned child still terminates its generation

- **WHEN** a UI generation is fenced and its executors do not cease within the
  first bounded window
- **THEN** the supervisor drops that generation's broadcaster handle and
  subscriber senders
- **AND** no further frame is emitted by that generation
- **BECAUSE** the primitive's contract is positive cessation of every
  generation-owned effect path, not the termination of a child process

#### Scenario: Fencing a Tmux generation does not terminate the server

- **WHEN** a Tmux generation's termination primitive is invoked
- **THEN** only the generation's owned `tmux` client invocations are terminated
- **AND** the tmux server and the operator's sessions are left running

#### Scenario: A negative fence does not strand any member

- **WHEN** a fence remains negative because cessation was not observed
- **THEN** every member of that generation still resolves through the guard's
  evidence order
- **AND** each member's admission quota is released
- **BECAUSE** outcome terminality does not require the fence to be positive

#### Scenario: A fenced submission resolves not_submitted

- **WHEN** the fence stops a submission before it produces any target-side effect
- **THEN** that unit's members resolve `not_submitted`
- **AND** they do not resolve `submission_unknown`

### Requirement: Transport Health as a Separate Axis

A transport SHALL report **health** as a level distinct from its own internal
write-readiness, carrying the instant it was first observed unreachable:

| State | Meaning |
|---|---|
| `Healthy` | the transport can reach its target |
| `Unreachable { since }` | the transport cannot observe or reach its target at all, first seen at `since` |

Health and internal write-readiness answer different questions. Readiness says
*when* declaring and writing is useful; health says *whether* it is possible at
all. A target that is busy and a target whose transport cannot reach it both
fail the transport's own readiness check, and only the first is a reason to
keep waiting rather than to eventually resolve the entry.

**A transport SHALL declare and write only when it is both `Healthy` and
internally ready.** Both checks are now entirely the transport's own —
neither is relay-facing. Healthy-but-unready leaves entries `queued` and
undeclared.

The transport SHALL determine health and report when it began; the relay SHALL
own the dwell threshold as `[delivery]` policy. A transport SHALL NOT reference a
relay interface to report it, per `Transport Module Boundaries`.

A member SHALL be resolved only after its target has been **continuously
unreachable past the configured threshold**, never on a single failed
observation. One observation that does not come back cannot distinguish a
transient failure from a departed target, and a bounce asserts something to the
sender that a wait does not.

This threshold SHALL NOT be read as a delivery timeout. No bound converts a
readiness wait into an outcome, because duration cannot substitute for an
observation that was never made. Here duration qualifies observations that were
made repeatedly, and sustained unreachability is itself evidence in a way that
sustained busyness is not.

Health SHALL gate write paths and SHALL NOT gate `look`. `raww` shares the
ordered mailbox and inherits the gate. A `look` SHALL NOT be rejected on
account of its target's health: a target is inspected precisely when something is
wrong with it, so refusing to even attempt the snapshot removes the diagnostic
exactly when it is needed.

Whether a snapshot then comes back is the transport's own affair, and this
requirement SHALL NOT be read as promising one. A tmux transport reports
`Unreachable` *because* its pane cannot be observed, and that is the same pane a
snapshot would be captured from, so the attempt fails on its own terms. An
operator gets the transport's real error instead of a policy refusal, which is
the diagnostic difference this requirement exists to preserve.

#### Scenario: A busy target keeps waiting

- **WHEN** a transport reports `Healthy` to itself but its own internal
  readiness check does not pass
- **THEN** its peeked entries stay `queued` and undeclared
- **AND** no elapsed duration resolves them

#### Scenario: A sustained-unreachable target resolves its members

- **WHEN** a transport reports `Unreachable` continuously past the configured
  threshold
- **THEN** its still-`queued` undeclared members resolve `not_submitted`
  directly, and its declared members resolve through the guard's evidence
  order, per `delivery-quiescence`'s `Mailbox Ordering and Cursor Lifecycle`
- **AND** their admission quota is released on that terminal transition

#### Scenario: A transient unreachability does not resolve anything

- **WHEN** a transport reports `Unreachable` and reports `Healthy` again before
  the threshold elapses
- **THEN** no member was resolved
- **AND** the members that were waiting are peeked, declared, and written
  normally once the transport's own readiness allows

#### Scenario: Health does not reject a look

- **WHEN** an operator looks at a target whose transport reports `Unreachable`
- **THEN** the request is dispatched to the transport rather than rejected on
  health grounds
- **AND** any failure returned is the transport's own snapshot failure

#### Scenario: Health determination carries no relay dependency

- **WHEN** a transport determines its own health
- **THEN** it references no `crate::relay` type

## ADDED Requirements

### Requirement: Neutral Delivery Protocol Crate Boundary

The system SHALL hold the vocabulary shared by both delivery call
directions — relay-to-transport for `look`, transport-to-relay for
`peek`/`declare`/`ack` — in a crate (or crate-internal module boundary) that both
sides depend on, promoted from the `src/transports/vocabulary` module
rather than
newly constructed.

This crate SHALL hold: mailbox entry and entry-kind representations, target
and consumer identity, consumer-generation binding, cursor position,
`PackingUnitId`, and the `peek`/`declare`/`ack` request and response
shapes, and doorbell subscription handles. It SHALL NOT hold `AsyncDeliveryTask`, `BundleMember`,
`TransportImpl`, or any `crate::relay` error type. `look`, startup, transport
generation fencing, concrete transport constructors, and `TransportImpl`
itself remain outside it.

**If either call direction still needs the other's concrete type to express
its contract, the inversion has not happened.** This requirement exists
specifically so the module-dependency direction the `Transport Module
Boundaries` requirement already forbids in one direction (transport
importing relay internals) does not silently re-appear in the other
direction as `peek`/`ack` are introduced (relay importing transport
internals to call them).

#### Scenario: The protocol crate compiles without relay or concrete-transport imports

- **WHEN** the neutral delivery protocol crate is built in isolation
- **THEN** it does not import `crate::relay`, `crate::acp`, `crate::tmux`,
  `crate::pty`, or `crate::transports::ui`
- **AND** it exposes the `peek`/`declare`/`ack` request/response types and
  mailbox vocabulary needed by both call directions

#### Scenario: Neither call direction needs the other's domain type

- **WHEN** the relay's `look` handler and a transport's delivery-loop
  executor are each implemented against the neutral crate
- **THEN** neither imports a concrete type owned by the other side to
  express its own request or response shape
