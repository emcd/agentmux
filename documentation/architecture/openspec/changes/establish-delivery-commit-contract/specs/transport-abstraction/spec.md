## MODIFIED Requirements

### Requirement: Transport Interface Contract

The relay delivery subsystem SHALL dispatch all agent delivery operations
through two non-blocking write methods defined on the `Transport` trait in
`src/transports/contract.rs`:

- `mailw` — structured relay message write. The relay SHALL populate routing,
  attribution, message body, timestamp, choice-decider, and quiescence fields
  before calling the transport. **The invocation seam SHALL be a batch**: the
  relay invokes with the set of envelopes it has authorized together, and the
  transport SHALL NOT buffer the batch, coalesce it with a later one, or wait for
  target readiness before submitting it. The transport SHALL render any
  transport-specific representation internally and resolve each member with a
  terminal `SingleDeliveryOutcome` derived from that member's packing-unit
  evidence.
- `raww(content: String, mode: RawMode, append_enter: bool)` — raw input write,
  carrying the explicit mode defined by the `transport-contracts` capability's
  `Relay raww operation contract`.

**The invocation is fallible.** The relay's admission quota reserves count and
bytes in the relay's own queue and nothing about a transport's channel, its live
worker generation, UI subscriber capacity, or any target resource. A transport
SHALL be permitted to refuse an invocation, and a refusal SHALL be treated as a
terminal evidence result rather than as a reclaim:

- the transport returns the batch **unchanged**, before partition → every member
  resolves `not_submitted`;
- side effects **cannot be excluded** → the affected unit's members resolve
  `submission_unknown`.

The relay SHALL NOT reclaim or retry in either case.

**A transport SHALL NOT wait.** Post-authorization execution SHALL NOT wait on
prompt readiness, target turn completion, target output, or an operator decision.
No authorized batch SHALL sit in a transport staging queue behind an in-flight
turn. A submission primitive that can block SHALL be supervised and
fenced/interruptible per the `Transport Generation Fencing and Termination
Authority` requirement.

Each transport type SHALL implement these methods in its own module. The relay
SHALL dispatch via a `TransportImpl` enum that delegates without dynamic
allocation, and SHALL submit uniformly for every target with no transport-type
routing fork in the delivery loop. `TransportImpl` has **five** variants — `Acp`,
`Tmux`, `Pty`, `Ui`, and `Pubsub` — and this contract applies to all of them.

`mailw` and `raww` SHALL be the relay's only delivery seam. The relay worker
SHALL NOT pre-render pane-envelope text before calling `mailw`; representation
rendering belongs to the receiving transport. The legacy synchronous methods —
`deliver`, `prepare_delivery`, and `raw_write` — and the types that existed
solely to serve them SHALL NOT be retained.

The trait methods SHALL be non-blocking at the relay boundary. On relay shutdown,
still-pending relay-owned members resolve `dropped_on_shutdown`; authorized
members resolve from evidence.

#### Scenario: ACP delivery via TransportImpl

- **WHEN** the relay authorizes a batch for an ACP target
- **THEN** it invokes `TransportImpl::Acp(t)` with the batch
- **AND** the ACP transport partitions it, renders pane-envelope text internally,
  and submits each unit as its own `session/prompt` request
- **AND** it does not park the batch behind an in-flight turn

#### Scenario: Tmux delivery via TransportImpl

- **WHEN** the relay authorizes a batch for a Tmux target
- **THEN** it invokes `TransportImpl::Tmux(t)` with the batch
- **AND** the Tmux transport partitions it into token-budget prompts and injects
  each separately
- **AND** it does not wait for pane quiescence, which the relay has already done

#### Scenario: UI delivery via TransportImpl

- **WHEN** the relay authorizes a batch for a `Ui` target
- **THEN** it invokes `TransportImpl::Ui(t)` with the same structured message
  data used for coder transports
- **AND** the UI transport emits the messages as relay stream events through its
  injected broadcaster closure
- **AND** no `Ui`/`Pubsub` delivery short-circuit appears in the dispatch path

#### Scenario: A transport refuses an invocation before partition

- **WHEN** a transport's write channel is full or closed, or its worker
  generation is dead
- **THEN** it returns the batch unchanged without partitioning it
- **AND** every member resolves `not_submitted`
- **AND** the relay does not return them to `Pending`

#### Scenario: Shutdown resolves pending members

- **WHEN** relay shutdown is requested
- **THEN** still-`Pending` relay-owned members resolve `dropped_on_shutdown`
- **AND** `Authorized` members resolve from evidence

#### Scenario: Startup never runs on an async runtime thread

- **WHEN** the relay invokes `Transport::startup` for any session type
- **THEN** it runs the call on a blocking thread rather than on a runtime worker
  thread, because `startup` is synchronous on the trait and every implementation
  of it is therefore permitted to block
- **AND** the relay SHALL NOT make that choice per session type, so that a
  transport acquiring a blocking startup step later inherits the guarantee
  rather than an assumption about what it used to do
- **AND** because such a call cannot be aborted, each transport's `startup`
  SHALL own the cleanup of anything it created, reaching its own conclusion even
  when the caller awaiting it has gone away

### Requirement: Transport Module Boundaries

ACP-specific delivery code SHALL reside in `src/acp/`. Tmux-specific delivery
code SHALL reside in `src/tmux/`. Pty-specific delivery code SHALL reside in
`src/pty/`. UI stream-broadcast delivery code SHALL reside in its own transport
module (`UiTransport`), not in the relay delivery subsystem.

The boundary SHALL distinguish four concerns that were previously conflated:

| Concern | Owner |
|---|---|
| **Queueing** — what is pending for a target, in what order, and for how long | **relay** |
| **Readiness scheduling** — which target to visit, in what order, and when to authorize | **relay** |
| **Readiness determination** — observing the target and deciding whether a handover can be taken now | **transport** |
| **Rendering and packing** — target representation and partition into packing units | **transport** |

Queueing and readiness scheduling SHALL move relay-side. Readiness determination,
rendering, and packing SHALL remain transport-owned.

**Readiness scheduling and readiness determination are different concerns and are
owned by different sides.** Scheduling is transport-agnostic: it reasons about
queues, order, and quota. Determination is transport-specific by nature and does
not generalise — a prompt regex over a pane tail is meaningless for ACP, whose
readiness is an earlier turn completing on the wire protocol with no snapshot to
inspect, and meaningless again for UI, whose readiness is subscriber
connectivity. Conflating them is what would put pane semantics inside the relay.

The relay SHALL learn readiness only as the level it reads through
`is_ready_for_handover`, refreshed by the transport-invoked notification closure
described in the `Transport Handover Capacity and Readiness` requirement. Only
the transport can render target text and count its tokens, so `prompt_tokens_max`
likewise remains an internal packing-unit limit invisible to the relay.

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

- **WHEN** a developer looks for the logic that splits a batch into packing units
- **THEN** they find it in the owning transport module
- **AND** the relay expresses its own limits only in envelope count and canonical
  payload bytes, never in rendered tokens

#### Scenario: UI target delivered through its transport, not a relay path

- **WHEN** the relay receives a delivery task for a `Ui` target
- **THEN** it dispatches through `TransportImpl::Ui` uniformly, with no
  transport-type routing fork
- **AND** no `TargetConfiguration::Ui | Pubsub` delivery arm or UI delivery
  short-circuit appears in the dispatch path

#### Scenario: Queueing lives relay-side

- **WHEN** a developer looks for the pending queue for a target
- **THEN** they find one relay-owned queue rather than a per-transport buffer
- **AND** no transport retains envelopes awaiting a readiness condition

#### Scenario: Readiness determination stays with the transport

- **WHEN** a developer looks for the logic that decides whether a target can take
  a handover now
- **THEN** they find it in the owning transport module
- **AND** `src/relay/delivery/` contains no prompt-regex matching, pane
  inspection, or cursor-column comparison
- **AND** the relay reads only the `is_ready_for_handover` level

### Requirement: Synchronous Delivery Completion

Each member of an authorized batch SHALL resolve with a terminal
`SingleDeliveryOutcome`; the relay worker maps that outcome onto its `SendResult`
(the outcome carries the transport-side type, not the relay `SendResult`,
preserving the no-relay-dependency invariant).

**Outcomes are per packing unit, not per batch.** If one unit submits and another
fails, their members SHALL receive different outcomes. A transport SHALL NOT
apply one outcome to every member of a batch.

Every member's outcome SHALL be **derived from its unit's immutable evidence
record**, never from live re-inspection at fan-out time.

The transport SHALL NOT drop a member without resolving it, and the relay-owned
guard SHALL terminalize any member the transport fails to resolve, selecting the
outcome by the guard resolution order defined in the `delivery-quiescence`
capability's `Delivery Authorization and Terminal Guard` requirement. This does
not block the relay request path: the send RPC returns `queued` at admission.

#### Scenario: Member outcome resolves through the relay worker

- **WHEN** the relay invokes a transport with an authorized batch
- **THEN** each member resolves with a terminal `SingleDeliveryOutcome`
- **AND** the relay worker maps that outcome onto its `SendResult` at the collect
  site, without the transport referencing any `crate::relay` type

#### Scenario: Differing outcomes across units of one batch

- **WHEN** unit 1 of a batch submits successfully and unit 2 fails
- **THEN** unit 1's members resolve `delivered`
- **AND** unit 2's members resolve `not_submitted` or `submission_unknown`
- **AND** neither result is applied to the other unit's members

#### Scenario: An earlier unit's success is not retracted

- **WHEN** a transport fails or panics while submitting a later unit
- **THEN** the members of already-submitted units keep their `delivered` outcome

#### Scenario: The guard resolves what the transport does not

- **WHEN** a transport returns without resolving some members
- **THEN** the relay-owned guard terminalizes them by its evidence order — the
  unit's record if one exists, `not_submitted` if the member was never bound to a
  unit, `submission_unknown` otherwise
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

### Requirement: Positive Activity Signal

Each transport whose target produces observable output SHALL expose a
cross-transport activity signal from a transport-native
**terminal-output-write** primitive. The signal is a monotonic `u64` generation
that advances when bytes flow to the target's terminal, independently of whether
captured content visibly changed.

**The relay consumes this signal**; no transport classifies on it. An advance
between two consecutive observations SHALL be treated by the relay as a positive
indication that the target is active, and SHALL suppress handover for that
iteration.

**Scope (terminal-output-write, not process-busy):** the field carries a marker
of bytes being written to the target's terminal. Its **absence SHALL NOT be
treated as a signal of any kind.** A target that is quiet may be hung, may be
awaiting an operator, or may be working silently, and nothing distinguishes them.

A transport that does not track activity, or whose primitive is unavailable,
SHALL populate the field with the constant `0`. A constantly-`0` signal can never
advance, so such a target is never suppressed on this basis.

#### Scenario: Tmux probe populates the activity signal from window_activity

- **WHEN** the Tmux probe observes and `#{window_activity}` returns a non-empty
  value
- **THEN** the resulting activity generation is the parsed `u64` epoch-seconds
  value of that marker

#### Scenario: Tmux probe falls back to 0 when window_activity is unavailable

- **WHEN** the Tmux probe observes and `#{window_activity}` is unavailable on the
  running tmux version
- **THEN** the resulting activity generation is `0`
- **AND** no advance is possible, so handover is never suppressed on this basis
  for that target

#### Scenario: Pty probe populates the activity signal from last_change_atomic

- **WHEN** the Pty probe observes
- **THEN** the resulting activity generation is the current value of
  `last_change_atomic` loaded with `Ordering::Acquire`

#### Scenario: Activity advance suppresses handover

- **WHEN** a target's activity generation advances between two consecutive relay
  observations
- **THEN** the relay does not authorize a batch for that target in that iteration
- **AND** the entry remains `Pending`

#### Scenario: Absence of activity produces no outcome

- **WHEN** a target's activity generation does not advance across any number of
  observations
- **THEN** no terminal outcome is produced on that basis
- **AND** the entry remains `Pending`, resolving only if it is later authorized,
  if its transport is positively observed torn down, or at relay shutdown

### Requirement: Transport-Internal Probe Seam for Testability

Each transport whose target can be observed SHALL expose an internal probe trait
that lets tests inject deterministic readiness observations. The probe trait
SHALL be transport-internal (not part of the `Transport` contract) and SHALL NOT
appear in `src/transports/contract.rs`.

The probe trait SHALL return the next observation on demand so tests can drive
the relay's readiness scheduling through specific sequences. Because no transport
classifies an observation into a terminal delivery state, no probe sequence
SHALL assert a terminal failure derived from observation.

#### Scenario: Probe trait is transport-internal

- **WHEN** a developer reads a transport's probe module
- **THEN** the trait is not re-exported from `src/transports/`
- **AND** the `Transport` trait in `src/transports/contract.rs` has no knowledge
  of probes

#### Scenario: No probe sequence asserts an observation-derived failure

- **WHEN** a transport's probe tests run
- **THEN** no sequence asserts a terminal delivery failure produced from an
  observation
- **BECAUSE** observations feed relay scheduling, and scheduling produces no
  terminal outcome

### Requirement: Pty Transport Implementation

The system SHALL provide a `PtyTransport` in `src/pty/transport.rs` that
implements the `Transport` trait and is wired into `TransportImpl::Pty`. The
transport SHALL own one `libghostty_vt::Terminal<'static, 'static>`, one
`portable_pty` master, one reader thread, and one delivery task. Because all
`libghostty_vt` types are `!Send + !Sync`, the terminal SHALL live on the
delivery thread and be reached from other threads through a `SnapshotRequest`
channel.

**Pty SHALL buffer, then write** — the ordering Tmux already uses. The transport
SHALL NOT write any member to the PTY master before the batch's partition is
recorded. Writing before the wait is what made a flush group's membership mutable
after its write, which is the defect behind `agentmux:issues/relay/62`.

**Pty members SHALL be singleton packing units** unless a future change genuinely
combines them into one write, because the transport writes each member with its
own `write_all` pair. Each unit's outcome SHALL be derived from its own
evidence, and one outcome SHALL NOT be applied to every member of a group.

No envelope SHALL be absorbed into a batch after that batch is authorized.

#### Scenario: Pty startup spawns the child PTY and installs effect handlers

- **WHEN** the relay calls `TransportImpl::Pty(t).startup(context)` for a
  Pty-backed bundle member
- **THEN** the transport opens a `portable_pty` master sized to the per-coder
  `cols` and `rows`
- **AND** spawns the configured child command with `COLORTERM=truecolor` and a
  `TERM` env-var value derived from the per-coder `term-protocol` field
- **AND** constructs a `libghostty_vt::Terminal` with the same dimensions and
  installs the canonical effect handlers
- **AND** spawns the reader thread and the delivery task
- **AND** the worker thread publishes `WorkerReadinessState::Available` AFTER
  successful `Terminal::new` + handler installation

#### Scenario: Pty startup does not wait for the worker to initialize

- **WHEN** `startup` has spawned the child, the worker thread, and the reader
  thread
- **THEN** it returns `TransportReadiness::Pending` immediately rather than
  waiting for the worker to report that it initialized
- **AND** the worker publishes `WorkerReadinessState::Available` when it has
  genuinely initialized, so `is_ready_for_handover` is what gates the handover
  and the return value carries no readiness answer
- **AND** a worker that never arrives is treated as a target that is never
  ready — the entry stays `Pending`, bounded in consequence by per-target
  admission quota rather than by a clock, exactly as for every other transport

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

#### Scenario: Pty submits an authorized batch immediately

- **WHEN** the relay invokes the Pty transport with an authorized batch
- **THEN** the transport partitions it into one unit per member and records the
  partition before writing any bytes
- **AND** writes each unit to the PTY master without waiting for quiescence
- **AND** resolves each member from its own unit's evidence

#### Scenario: No envelope is absorbed into an authorized batch

- **WHEN** a new envelope is admitted while a batch for the same target is
  authorized
- **THEN** it forms part of a later batch
- **AND** it is not added to the in-flight batch
- **BECAUSE** a mutable batch membership is what allowed one outcome to be
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
- **AND** joins the reader thread handle and the worker thread handle

## ADDED Requirements

### Requirement: Packing Units and Typed Submission Evidence

Transports SHALL partition an authorized batch into packing units before
producing any target-side effect, and SHALL resolve every member from typed,
per-unit evidence.

A **batch** is the unit of authorization. A **packing unit** is the unit of
target-side submission. They are not the same, and a batch SHALL NOT be treated
as one atomic target write.

**Partition SHALL be fixed and exact before the first target-side effect.** Every
member belongs to exactly one unit, order is preserved, and no member is added to
a batch after authorization. The transport SHALL assign `PackingUnit ID`s at
partition and record the partition to the relay-owned guard before producing any
effect. There SHALL be no absorption across batches.

An envelope whose rendered size alone exceeds the packing budget SHALL form its
own unit.

**Identities are explicit and immutable.** A `Batch ID` and `Member ID` are
assigned at authorization, a `PackingUnit ID` at partition, and none is ever
reassigned. Each authorization additionally carries a stable attempt ID. The full
partition SHALL be retained for the batch's lifetime so resolution can attribute
every member to the unit that carried it.

**Submission evidence SHALL be typed**, not inferred from an error string:

| Evidence | Meaning |
|---|---|
| `Submitted` | the target-side primitive positively reported success |
| `NotSubmitted` | positive evidence that no side effect occurred |
| `SubmissionUnknown` | side effects cannot be excluded |

An undifferentiated error SHALL map to `SubmissionUnknown`, never
`NotSubmitted`. Only a primitive that can prove nothing was written may report
`NotSubmitted`. A Tmux paste is a body write followed by Enter and a Pty unit is
multiple `write_all` calls, so both can fail after partial effect.

**A member that was never bound to a packing unit SHALL resolve `not_submitted`.**
Because the partition is recorded before the first target-side effect, an unbound
member provably could not have been submitted, whatever ended the attempt —
refusal, panic, or cancellation. The discriminator is unit binding, not the manner
of failure.

**Unit evidence SHALL be recorded atomically before any member fan-out.**
Evidence is established per unit but guards terminalize per member, and resolving
members one at a time from live state is not safe: a resolver that panics halfway
through fan-out would leave some members `delivered` while their siblings were
terminalized `submission_unknown` from identical target-side evidence. The
sequence SHALL therefore be:

1. the unit's submission produces one **immutable unit evidence record**, written
   before any member outcome is derived;
2. every member's terminal outcome is **derived from that record**;
3. a panic during fan-out **resumes from the recorded unit result** rather than
   inventing `submission_unknown` for the remainder.

**Per-transport terminal evidence and observation windows:**

| Transport | `delivered` evidence | Window closes at |
|---|---|---|
| Tmux | `inject_literal_text` returns `Ok` | submission; a later pane death is target-health observability |
| Pty | the unit's `write_all` pair to the master succeeds | submission; a later child exit is target-health observability |
| ACP | write and flush of the complete newline-delimited `session/prompt` JSON-RPC request succeeds | that framed write; the turn's later completion, permission requests, or connection close are target-health observability |
| UI | the broadcast is accepted by at least one live subscriber | submission |

**Submission success terminalizes `delivered` on every transport.** A later
positively observed exit or close SHALL be recorded as target health, not as a
second delivery outcome for an already-resolved member. There is no `target
failed` delivery outcome.

A partially-succeeded write within one packing unit SHALL yield
`submission_unknown` for that unit's members: the bytes may be on the target in
truncated form, which is neither delivery nor absence.

#### Scenario: Record the partition before any effect

- **WHEN** a transport receives an authorized batch
- **THEN** it partitions the batch and records the partition to the guard
- **AND** it produces no target-side effect before that record exists

#### Scenario: An unbound member resolves not_submitted

- **WHEN** a transport fails, refuses, or panics before binding any member to a
  packing unit
- **THEN** every member resolves `not_submitted`
- **BECAUSE** the partition precedes the first effect, so nothing could have been
  submitted

#### Scenario: An undifferentiated error is not evidence of absence

- **WHEN** a submission primitive returns an error that cannot prove zero bytes
  left
- **THEN** the unit's members resolve `submission_unknown`
- **AND** they do not resolve `not_submitted`

#### Scenario: A partial write within a unit is unknown

- **WHEN** a unit's body write succeeds and its terminating write fails
- **THEN** that unit's members resolve `submission_unknown`

#### Scenario: Siblings share one outcome from one record

- **WHEN** a resolver panics partway through the member fan-out of a unit
- **THEN** the remaining members resolve from the recorded unit evidence
- **AND** every member of that unit carries the same outcome

#### Scenario: An oversized envelope forms its own unit

- **WHEN** a single envelope's rendered size exceeds the packing budget
- **THEN** it is partitioned into a unit of its own
- **AND** its outcome is not shared with any other member

#### Scenario: A post-submission exit does not resolve a member twice

- **WHEN** a unit's submission succeeds and the target process later exits
- **THEN** the unit's members remain `delivered`
- **AND** the exit is recorded as target-health observability

### Requirement: Transport Generation Fencing and Termination Authority

A transport generation SHALL be **torn down and fenced before its replacement
begins**, so an old generation cannot submit after its `Authorized` entries were
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

### Requirement: Transport Handover Capacity and Readiness

Three quantities that were previously conflated under "capacity" SHALL be
separate:

| Quantity | Owner | Purpose |
|---|---|---|
| **Admission quota** | relay | how much may be queued per target and relay-global; enforced at admission |
| **Maximum handover dimensions** | transport, static | the largest batch a transport will accept |
| **Acceptance capacity** | transport, dynamic | whether it can accept right now; surfaced as `is_ready_for_handover` |

All relay-facing quantities SHALL be expressed in units the relay can evaluate
without packing: **envelope count and canonical payload bytes**, where canonical
bytes means the serialized envelope payload the relay already holds, not rendered
target text. Declaring them in tokens would be circular, since only the transport
can render and count those.

`is_ready_for_handover` SHALL be **level-triggered**, readable on demand, and the
transport contract's **only** readiness predicate. It SHALL have no default
implementation: a default of `true` would authorize a busy target, and a default
of `false` would strand it permanently, so a transport answers for itself or does
not participate in delivery.

An earlier `is_ready` answered the weaker question of whether a transport's
machinery existed — Tmux answered it unconditionally true, and ACP and Pty
counted `Busy` as ready — which is why it could not serve handover readiness. It
has been **removed from the contract rather than redefined**, because two
readiness predicates were confusable precisely under a name that does not say
what it is ready *for*. A transport MAY keep an equivalent lifecycle predicate
privately, and Pty does, gating its `OutputView` on the runtime existing so that
`look` still reaches a target that is mid-turn.

`is_ready_for_handover` is **advisory** — a stale reading yields a fallible
invocation, not a guarantee.

**No transport → relay back-edge.** The relay calls transports; transports SHALL
NOT know relay interfaces. Where a transport signals upward it SHALL invoke an
opaque closure the relay provided at construction, the pattern `PtyTransport`
already uses for `mirror_state`. Correctness SHALL NOT depend on that
notification: it is an edge hint, the authoritative state is the level the relay
reads, and authorization is a relay-local transition. A lost wakeup delays a
delivery until the next poll; it cannot lose one or resolve it without evidence.

#### Scenario: Readiness is readable as a level

- **WHEN** the relay needs to decide whether handover is useful
- **THEN** it reads `is_ready_for_handover` directly
- **AND** does not rely on having observed a transition

#### Scenario: A lost notification only delays

- **WHEN** a transport's readiness notification is not observed by the relay
- **THEN** the relay discovers the change on its next poll
- **AND** no message is lost or resolved without evidence

#### Scenario: A transport signals upward through an injected closure

- **WHEN** a transport needs to notify the relay of a readiness change
- **THEN** it invokes a closure the relay supplied at construction
- **AND** it does not reference any `crate::relay` type

#### Scenario: Handover dimensions are declared in relay-evaluable units

- **WHEN** a transport declares its maximum handover dimensions
- **THEN** they are expressed in envelope count and canonical payload bytes
- **AND** not in rendered tokens

### Requirement: Transport Health as a Separate Axis

A transport SHALL report **health** as a level distinct from handover readiness,
carrying the instant it was first observed unreachable:

| State | Meaning |
|---|---|
| `Healthy` | the transport can reach its target |
| `Unreachable { since }` | the transport cannot observe or reach its target at all, first seen at `since` |

Readiness and health answer different questions. Readiness says *when* a handover
is useful; health says *whether* one is possible. A target that is busy and a
target whose transport cannot reach it both fail a readiness check, and only the
first is a reason to wait.

A delivery attempt SHALL require both: the transport reports `Healthy` **and**
reports `is_ready_for_handover`. Healthy-but-unready leaves the member `Pending`.

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
ordered delivery channel and inherits the gate. `look` SHALL remain available on
an unreachable target and SHALL carry the health level in its response metadata:
a target is inspected precisely when something is wrong with it, so withholding
the snapshot removes the diagnostic exactly when it is needed.

#### Scenario: A busy target keeps waiting

- **WHEN** a transport reports `Healthy` and does not report
  `is_ready_for_handover`
- **THEN** its member stays `Pending`
- **AND** no elapsed duration resolves it

#### Scenario: A sustained-unreachable target resolves its members

- **WHEN** a transport reports `Unreachable` continuously past the configured
  threshold
- **THEN** its still-`Pending` members resolve through the guard's evidence order
- **AND** their admission quota is released on that terminal transition

#### Scenario: A transient unreachability does not resolve anything

- **WHEN** a transport reports `Unreachable` and reports `Healthy` again before
  the threshold elapses
- **THEN** no member was resolved
- **AND** the members that were waiting are authorized normally once readiness
  allows

#### Scenario: Look survives an unreachable target

- **WHEN** an operator looks at a target whose transport reports `Unreachable`
- **THEN** the snapshot request is served rather than refused
- **AND** the response carries the health level and how long it has held

#### Scenario: Health determination carries no relay dependency

- **WHEN** a transport determines its own health
- **THEN** it references no `crate::relay` type
- **AND** the relay supplies the dwell threshold rather than the transport

## REMOVED Requirements

### Requirement: Three-State Delivery Classifier

**Reason:** the classifier's `wedged` state infers a terminal failure from the
absence of change in rendered content, and its `unresponsive` state does the same
from the absence of output. Both are the unsound inference this change exists to
remove. The requirement itself already recorded that `wedged` is unsound and
carried Pty as a named temporary exception pending a Pty readiness bound. This
change retires the exception's premise rather than satisfying it: no readiness
bound is supplied anywhere, and Pty resolves from typed submission evidence once a
batch is authorized, so the classifier's terminal path is not needed.

The remaining sound part — that positively observed activity suppresses handover —
is retained and relocated to the `Positive Activity Signal` requirement, where it
is consumed by relay scheduling rather than by a transport classifier.

**Migration:** no `format-version` bump and no persisted-state migration.
Deliveries that previously resolved `Failed` with a wedge reason code, or
`Timeout` at a prime or readiness bound, now resolve from typed submission
evidence when a batch was authorized, and otherwise remain `Pending` until the
target becomes ready. Senders receive a terminal-outcome receipt in every
non-delivered case, but a message still waiting has no outcome and therefore no
receipt; that condition is reported through the `delivery-quiescence` capability's
undelivered-queue inscriptions instead.

### Requirement: Prime Timeout Envelope Field

**Reason:** the field exists to carry a bound on "no observable output has been
produced", which is an absence. With all readiness waiting moved relay-side and
left unbounded by design, no transport performs a prime wait, so there is nothing
for the field to bound.

**Migration:** `DeliveryEnvelope.prime_timeout_ms` is deleted outright, along with
the per-coder keys that populated it. A `coders.toml` that still sets
`[coders.<id>.tmux].prime-timeout-ms` fails load on existing unknown-field
validation; an operator SHALL delete the line. No value preserves the prior
behavior, because the behavior is gone.

### Requirement: ACP Prime Timeout Envelope Field Consumption

**Reason:** it specifies ACP's consumption of a field this change deletes, and
its behavior on fire — latching readiness to `Unavailable` and signalling
respawn from an elapsed timer — is precisely the inference from absence being
retired. ACP's turn lifecycle is post-submission target state under the new
contract and does not hold its members' delivery outcomes open.

**Migration:** the ACP prime timer and its `acp_turn_timeout` reason code are
deleted. An ACP delivery now resolves at its framed write per the
`Packing Units and Typed Submission Evidence` requirement, and a turn that never
completes no longer holds a delivery outcome open to be timed out.

### Requirement: Generalized Wedge/Prime State Machine

**Reason:** it is the shared implementation of the two absence-derived
classifications this change removes. With readiness scheduling relay-owned, no
transport runs a quiescence wait, so there is no shared state machine for
transports to drive.

**Migration:** `src/transports/quiescence.rs` and the `WedgeProbe` trait are
deleted. Transports retain internal observation probes for the relay's readiness
scheduling per the `Transport-Internal Probe Seam for Testability` requirement,
but those probes produce observations only and never a terminal delivery state.
