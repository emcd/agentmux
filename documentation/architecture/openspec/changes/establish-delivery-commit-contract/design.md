## Context

Agentmux has three transport-local timers that each decide a delivery failed
because nothing changed: wedge detection, the prime timeout, and the readiness
bound. The live `Delivery Classifier` requirement already rejects that rule —
*"only a positively observed terminal event … is sound evidence of failure, and
an unchanged screen is not"* — four paragraphs above a timer that implements it.

Underneath them, **commitment is implicit, differs per transport, and is not even
a single event within one transport.**

- `mailw` is an enqueue, not a commit: it "buffers the envelope on its own ordered
  channel, coalesces it with contiguous envelopes during its quiescence wait"
  (`src/transports/contract.rs:105-110`).
- Pty writes every envelope to the pty master before its wait
  (`src/pty/delivery.rs:307`, `:356-369`); ACP dispatches `client.prompt(...)` at
  `src/acp/transport.rs:1189` and starts its timer at `:1199`; only Tmux writes
  after its wait.
- A single flush group is split into **multiple independently fallible target
  submissions**. Tmux partitions into token-budget prompts and injects each
  separately (`src/tmux/transport.rs:659-697`); ACP does the same via
  `batch_envelope_groups`; Pty writes members one at a time in a loop
  (`src/pty/delivery.rs:356-369`).

So "the delivery was committed" is not one fact. It is at least four: when the
relay may start, when it stops owning the message, when each target-side effect
occurs, and what any of that proves.

`agentmux:issues/relay/62` is the cost of leaving that implicit: a Pty flush
group's membership is mutable *after* its write, and `send_group_outcomes`
(`:847`) resolves every member identically — some committed, some not, all
reported the same. The red test that pinned this was deleted along with the
transport-owned wait, leaving the defect currently unproven either way. Wedge
detection masks it by resolving
groups in ~150 ms, so retiring the timers widens the window; the fix and the
retirement cannot be sequenced apart.

**A first version of this design collapsed those four events into one word
("handover") and was rejected in review for exactly that.** The model below
separates them.

## Goals / Non-Goals

**Goals:**

- No transport decides a delivery failed from what a pane displays or how long it
  has been quiet.
- Exactly one linearization point per message, after which the relay never
  reclaims it, and before which cancellation is free.
- Any member that resolves does so exactly once in a surviving relay process,
  including when a transport panics; every *authorized* member resolves within a
  bound, while a *pending* one may wait indefinitely.
- Outcomes state what is actually known, distinguishing positive evidence of
  delivery, positive evidence of non-submission, and absence of evidence.
- Message loss of the relay/62 class becomes unreachable rather than patched.

**Non-Goals:**

- Crash recovery. 0.9.0 guarantees hold for a **surviving relay process and
  graceful shutdown** only. See *Decision 8*.
- Durable queues, `fetch`/mailbox retrieval (`agentmux:todos/runtime/23`), or
  pub-sub fan-out. This builds substrate they need and implements none of them.
- Exactly-once delivery. See *Decision 6*.
- Running transports as separate processes. The boundary is shaped for it.

## Decisions

### Decision 1 — Four events, one linearization point

| Event | Owner | Atomic? | Reversible? |
|---|---|---|---|
| **Admission** — accept into the queue, reserve capacity, return `queued` | relay | yes | yes (pre-commit) |
| **Authorization** — `Pending` → `Authorized` on the queue entry | relay | **yes — the linearization point** | **no** |
| **Submission** — one packing unit produces a target-side effect | transport | per unit | no |
| **Resolution** — each member resolves once, from recorded facts | transport → relay | per member | no |

**Authorization is a relay-local state transition on the relay's own queue
entry.** It is not a call, not a handshake, and does not depend on the transport
observing anything — which is why it is trivially atomic and why there is no
acceptance race. Cancellation competes only with this transition, and it competes
relay-locally.

After authorization the relay has irrevocably transferred delivery
responsibility. It never reclaims, never retries, and never asserts non-delivery
*by inference*. Positive evidence of non-submission remains reportable — see
*Decision 7*.

The relay then invokes the transport, and **that invocation is fallible.** An
earlier draft declared it non-rejectable on the grounds that the relay had
reserved capacity for it. That reasoning was circular: the relay's admission
quota reserves count and bytes **in the relay's own queue**, and nothing about a
transport's channel, its live worker generation, UI subscriber capacity, or any
target resource. `mailw` legitimately fails today — Tmux resolves `channel_full`
when its write channel is full or closed (`src/tmux/transport.rs:159-178`), ACP
has equivalent paths, and Pty can find a closed worker channel.

**A fallible invocation does not require an acceptance protocol, because
authorization already ended cancellation and retry.** A post-authorization
refusal is therefore a *terminal evidence result*, not a reclaim:

- the transport returns the unit **unchanged** → members resolve **not
  submitted**, which soundly asserts non-delivery on positive evidence;
- side effects **cannot be excluded** (partial write, panic, lost channel) →
  members resolve **submission unknown**.

The relay never reclaims in either case, so there is still no race to adjudicate.
This is strictly simpler than the infallible-acceptance model it replaces, and
honest about a failure mode that ships today.

If infallible acceptance is ever genuinely required, it needs
transport-generation-bound acceptance credits or reservation tokens. A count/byte
admission quota is not that mechanism, and this design does not pretend otherwise.

#### All waiting happens before authorization

Retiring every absence timer removes the only thing that previously bounded a
stuck delivery. The two sides of authorization are bounded differently, and
deliberately so: a `Pending` member waits as long as its target takes, because
elapsed time spent waiting for readiness is not evidence about the target; an
`Authorized` member is bounded by `[delivery].submission-timeout-ms`, because
that clock measures our own supervised code rather than the target's behavior. A
ledger records state without manufacturing a terminal event while the owner is
alive and blocked. An earlier draft left Pty buffering,
waiting, then writing *after* authorization, which would have made an authorized
member hang indefinitely with no bound anywhere in the system.

So the rule is structural, not incidental:

- **All target-readiness and quiescence waiting happens in `Pending`**, before
  authorization: prompt-readiness matching, quiescence observation, and — on ACP
  — completion and operator-choice resolution of an **older** turn. This says
  *when* the waiting happens, not who evaluates it. The relay owns the wait
  because it owns the queue; each transport owns the *determination* and reports
  it through `is_ready_for_handover`, since readiness is transport-specific by
  nature and does not generalise across a pane, a wire protocol, and a subscriber
  list (*Decision 3*).
- **Authorization starts exactly one immediate submission attempt.**
  Post-authorization execution SHALL NOT wait on prompt readiness, target turn
  completion, target output, or an operator decision.
- **No `Authorized` batch may wait in a relay or transport staging queue.**
  Authorization either synchronously refuses or synchronously starts one
  supervised submission executor. This is normative because ACP's `mailw` today
  only `try_send`s into a transport channel (`src/acp/transport.rs:387-410`),
  where a batch can sit behind an in-flight turn before partition — which is a
  post-authorization wait wearing a queue's clothing.

**The ACP wording above is a causality correction, not a detail.** An earlier
draft said ACP turn completion and operator-choice pauses happen in `Pending`
before authorization. That is impossible for the *current* batch: permission
requests arrive only because the request was written
(`src/acp/client.rs:602-610`, `:711-799`), and completion arrives later still
(`:615-630`). Those events are *caused by* the submission they were supposed to
precede. The correct rule:

- an **older** turn's completion and choices gate authorization of the **next**
  batch — they are readiness;
- the newly authorized batch's own turn lifecycle is **post-submission target
  state** and does not hold its members' delivery outcomes open.
- Any submission primitive that can block SHALL be supervised and
  fenced/interruptible, and post-authorization execution **SHALL** be bounded by
  `[delivery].submission-timeout-ms`. This was written as a *may* in an earlier
  draft, which left the model without a liveness trigger: every other trigger the
  guard has is an event, and an executor that stays alive and blocked produces
  none of them, so its quota would leak and its target's FIFO would stay blocked
  forever. Operator confirmed making it mandatory on 2026-08-04.

  On elapse the relay **initiates the generation fence and terminalizes nothing
  at that moment**. Unit evidence stays admissible through both bounded fence
  windows, and every still-unresolved member is terminalized through the guard's
  evidence order at the verdict — the single resolution cut. Quota and
  outcome-level barriers release with that terminalization; the target's FIFO,
  raw barrier, and replacement release only on a **positive** verdict, so a
  negative one leaves that target fail-stop while every other target keeps
  progressing. The bound is an **execution
  watchdog over the relay's own supervised code**: it states that our execution
  overran the time we allow it, not that the target failed. That is what makes it
  categorically different from the timers being retired, which inferred target
  failure from an unchanged screen. It yields `submission_unknown` rather than a
  failure spelling, because not knowing is what actually happened.

This is why the split is where the bound belongs. A member waiting in `Pending`
holds nothing but its own queue slot, so waiting indefinitely costs only quota,
which admission already reserves and bounds. A member waiting in `Authorized`
holds its target's FIFO, an executor, and a transport generation, so leaving that
side unbounded stalls work that has nothing to do with the stuck member.

### Decision 2 — Batch versus packing unit

A **batch** is the unit of authorization. A **packing unit** is the unit of
target-side submission. They are not the same, and a batch is *not* one atomic
target write.

- The partition of a batch into packing units SHALL be **fixed and exact before
  the first target-side effect**: every member belongs to exactly one unit, order
  is preserved, and no member is added to a batch after authorization.
- No absorption across batches. This is the rule that makes relay/62 unreachable.
- An envelope whose rendered size alone exceeds the packing budget forms its own
  unit. Tmux already does this (`src/tmux/transport.rs:650-651`).
- **Outcomes are per unit, not per batch.** If unit 1 submits and unit 2 fails,
  unit 1's members and unit 2's members get different outcomes. Tmux already
  implements this correctly (`:678-696`); Pty does not, and this change fixes it.

**The invocation seam is the Batch, and partition is the transport's first
action.** The relay invokes with a `Batch`; the transport deterministically
partitions it into `PackingUnit`s **before any side effect**, assigning
`PackingUnit ID`s at that moment and recording the partition to the guard. This
resolves what a "rejected invocation returns the unit unchanged" meant when no
unit yet existed:

- **Refusal before partition** — a full or closed channel, a dead worker
  generation — returns the Batch unchanged and terminalizes **every** member
  `not_submitted`. Nothing was partitioned, so nothing could have taken effect.
- **Failure after partition** — per-unit evidence applies, per the table below.

**Identities are explicit and immutable.** A `Batch ID` and `Member ID` are
assigned at authorization, a `PackingUnit ID` at partition, none ever reassigned;
each authorization additionally carries a stable attempt ID (*Decision 8*).
Authorizing a batch authorizes every member in it atomically — there is no
partially-authorized batch. The full partition is retained for the batch's
lifetime so that resolution can attribute every member to the unit that carried
it. **Pty members are singleton units** unless a future change genuinely combines
them into one write; today Pty writes each member with its own `write_all` pair
(`src/pty/delivery.rs:356-369`), so one-unit-per-member is what the code does.

**Submission evidence is typed**, not inferred from an error string:

| Evidence | Meaning |
|---|---|
| `Submitted` | the target-side primitive positively reported success |
| `NotSubmitted` | positive evidence no side effect occurred |
| `SubmissionUnknown` | side effects cannot be excluded |

An undifferentiated error maps to `SubmissionUnknown`, never `NotSubmitted`. This
matters concretely: a Tmux paste is a body write followed by Enter, and a Pty
unit is multiple `write_all` calls, so both can fail *after* partial effect. Only
a primitive that can prove nothing was written may report `NotSubmitted`.

**Unit evidence is recorded atomically before any member fan-out.** Evidence is
established per *unit*, but guards terminalize per *member*, and resolving
members one at a time from live evidence is not safe: a resolver that panics
halfway through fan-out would leave some members `delivered` while the supervisor
terminalized their siblings `submission_unknown` — from identical target-side
evidence. So the sequence is fixed:

1. the unit's submission produces one **immutable unit evidence record**, written
   before any member outcome is derived;
2. every member's terminal outcome is **derived from that record**;
3. a panic during fan-out **resumes from the recorded unit result** rather than
   inventing `submission_unknown` for the remainder.

This is what makes unit-level attribution and receipts deterministic, and it is
the difference between "one outcome per unit" as an intention and as a property.

Queueing (what is pending, in what order) is transport-agnostic and moves
relay-side. Rendering and packing stay transport-owned. The live
`Transport Module Boundaries` requirement does not currently distinguish queueing
from quiescence scheduling and packing, so it needs sharpening rather than a
rename; operator has agreed.

### Decision 3 — Readiness is level-triggered and advisory

The relay needs to know when handing over is *useful*, not when it is *permitted*.

- Add a **level-triggered** `is_ready_for_handover` capacity/readiness state, and
  make it the transport contract's only readiness predicate. The earlier
  `is_ready` could not serve — Tmux answered it unconditionally true, and ACP and
  Pty counted `Busy` as ready — so it is **removed rather than redefined**. Two
  predicates were confusable exactly because "ready" does not say what for; each
  transport keeps whatever lifecycle predicate it needs privately instead.
- The injected closure (*Decision 4*) delivers only an **edge**. The relay
  therefore MUST subscribe-before-check or re-check after subscribing, MUST poll
  at a bounded cadence as a backstop, and MUST re-read the level on every
  notification, admission, and completion.
- **If readiness changes between check and authorization, the invocation may
  fail**, and that resolves the unit as not-submitted or submission-unknown per
  *Decision 1*. Stale readiness can therefore change a delivery's **fate**, not
  merely its latency. An earlier draft claimed otherwise by leaning on infallible
  acceptance; with that removed, the honest statement is that readiness is a
  scheduling hint whose staleness produces an evidence-based terminal outcome
  rather than a lost or misreported message.

That is still enough to avoid a lease, token, or reject/ack protocol, because
authorization — not acceptance — is the linearization point. A lost wakeup delays
a delivery until the next poll; it cannot lose one or resolve it without evidence.

### Decision 4 — No transport → relay back-edge

The relay calls transports. Transports must not know relay interfaces. Where a
transport signals upward it invokes an opaque closure the relay **provided** at
construction — the pattern `PtyTransport` already uses for `mirror_state`, built
relay-side over `set_worker_readiness` (`src/pty/transport.rs:59-61`).

Combined with *Decision 3*, correctness never depends on the back-edge: the
notification is an edge hint, the authoritative state is the level the relay
reads, and authorization is a relay-local transition. This is what keeps the
boundary one-directional and representable over a wire.

### Decision 5 — Capacity is contract; the relay does not bound its own patience

Capacity and residency were previously filed as open questions. Both are
load-bearing and are settled here: capacity becomes contract, and residency is
deleted outright.

**Three distinct quantities were previously conflated under "capacity" and are
now separate:**

| Quantity | Owner | Purpose |
|---|---|---|
| **Admission quota** | relay | how much may be queued per target and relay-global; enforced at admission |
| **Maximum handover dimensions** | transport, static | the largest batch a transport will accept, in relay-evaluable units |
| **Acceptance capacity** | transport, dynamic | whether it can accept *right now*; surfaced as `is_ready_for_handover`, and **advisory** — a stale reading yields a fallible invocation per *Decision 1*, not a guarantee |

All relay-facing quantities are expressed in units the relay can evaluate without
packing: **envelope count and canonical payload bytes**, where canonical bytes
means the serialized envelope payload the relay already holds, not rendered
target text. Declaring them in tokens would be circular, since only the transport
can render and count those. `prompt_tokens_max` remains an **internal
packing-unit limit**, invisible to the relay, governing *Decision 2*'s partition
rather than the batch size. An envelope exceeding the maximum handover dimensions
on its own is rejected at admission rather than queued unsendable.

**Size and scheduling policy:**

- Per-target and relay-global bounds, both enforced.
- Capacity is **atomically reserved at admission, before `queued` is returned**,
  and released at the member's terminal transition — whether that is a pre-commit
  drop or a post-commit resolution.
- Scheduling is FIFO per target, where FIFO means worker-enqueue linearization:
  mail and raw reach a target through one keyed worker, and the order that
  worker's channel establishes is the order delivered. A request may reserve
  admission before another and still lose the enqueue race, so admission order is
  not delivery order and only the latter is guaranteed.
- **Across targets, nothing is scheduled, and that is deliberate.** Each target
  has its own worker; the relay does not arbitrate between them.

  An earlier revision of this document specified byte-budgeted round-robin with a
  configured quantum, per-visit credit, and debit-at-authorization. It is
  withdrawn. The quantum was required to be at least the largest permitted
  handover byte component while batch formation was already capped at that same
  component, so one quantum always afforded a full batch and the credit could
  bind only on a second batch within a visit — and visits existed only because a
  rotation did. The budget was there to be fair within a rotation; the rotation
  was there to allocate the budget.

  The withdrawn design had already caught this defect one level down and not
  recognised it. It carried a deficit counter, then deleted it on the reasoning
  that a carry-over rule plus the anti-monopoly cap fairness requires produced
  two incompatible spend limits on one quantity. The quantum stands in exactly
  that relation to the handover maxima — an outer limit constructed so it can
  never bind — and the same argument was not applied to it. Worth recording,
  because a visible self-correction reads as evidence that the neighbourhood was
  examined, and here it was not: the correction was evidence about the paragraph
  it appeared in and nothing more.

  **Targets do contend — that is not an argument for restoring it.** Tmux targets
  in a bundle share one tmux server and socket; ACP bootstrap enters a shared
  blocking pool; and a transport whose write seam blocks occupies a
  delivery-runtime worker thread. A global byte quantum measures none of these:
  not runtime occupancy, not channel slots, not tmux-server capacity. A
  resource-grounded policy would be denominated per shared resource and would owe
  a throughput or fairness objective, which nothing requires today. And a
  transport blocking the write seam violates the non-blocking handover contract —
  that is a defect to repair at its source, not a load to schedule around.
  Scheduling on top of it would bound a symptom and leave the cause.
- Admission quota is released through the guard's terminal transition
  (*Decision 6*) rather than by the collector, so a panicked task cannot leak it.
- The policy lives in relay configuration, not `coders.toml`, because it is a
  property of the relay's own queue rather than of any coder.

**There is no residency bound, and no `expired` outcome.**

An earlier draft of this change carried `residency-ms`: a bound on how long a
`Pending` member could wait before resolving `expired`. It was inherited from the
predecessor change rather than re-derived, which meant it was never subjected to
the test this change exists to apply. Under that test it fails.

Ask what elapsing *proves*. For `submission-timeout-ms` it proves that our own
supervised code overran the time we allot it — directly observed, and reported as
`submission_unknown`, which states our ignorance rather than the target's
condition. For residency it proves only that a target has not become ready yet,
and it is used to conclude that a message should not be delivered. That is
inference from absence, retired at the transport and reintroduced at the relay
under a calmer name.

The decisive point is that expiry is not a report. It terminalizes the member and
releases its quota, so the entry is gone: a message that would have landed when
the agent finished a long turn does not land. **We would be dropping mail to keep
a guarantee sentence true.** And the sentence cannot be honestly kept, because we
do not know how long a multi-round agent turn runs — any bound we pick is a guess
about someone else's work, and every message it expires is a message the guess
got wrong.

Residency's three apparent jobs have owners that do not require it:

| Apparent job | Real owner | Basis |
|---|---|---|
| Bound queue memory | admission quota, count and bytes, per target and relay-global | positive accounting, enforced before `queued` is returned |
| Resolve mail to a dead target | `transport_unavailable` | positively observed terminal lifecycle |
| Unstick a live but blocked target | operator teardown | positive action |

Only the fourth job is real, and it is the guarantee. "Resolves exactly once"
turns out to be three claims wearing one sentence, and only one of them is lost:

- **Uniqueness** — no member ever produces two terminal outcomes. This is the
  terminal CAS of *Decision 6*, and it is untouched.
- **Bounded completeness for `Authorized` members** — every authorized member
  reaches a terminal outcome within `submission-timeout-ms` plus twice the fence
  observation budget, on a positive and a negative verdict alike. The execution
  watchdog and the single resolution cut are what make this true, and it is also
  untouched.
- **Completeness for `Pending` members** — this is the one residency was propping
  up, and it is dropped. Nothing bounds a pending member.

Collapsing the three into one sentence is what made residency look load-bearing:
the sentence was false without it, so a timer appeared to be the fix. Separating
them shows the timer was buying only the third claim, which is the one we cannot
honestly make.

The honest statement, which replaces it: **every accepted message resolves at most
once, on delivery, on submission evidence, on a positively observed terminal
target lifecycle, on sustained unreachability past the dwell, or at relay
shutdown.** A message queued for a live target that
never becomes ready remains `Pending` indefinitely, and that is correct — the
target may still become ready, and the relay has observed nothing that says
otherwise. Sends are asynchronous by contract, returning `queued` without blocking
the caller, so such a member holds no caller and no thread; it holds one queue
slot, already reserved and already bounded.

Restoring completeness is a mailbox problem, not a timer problem. Client-acknowledged
receipt advancing a read cursor makes an undelivered message a durable fact rather
than a wait the relay is running out of patience with. That is tracked in
`agentmux:todos/runtime/23` and deliberately not attempted here.

**Compensating observability, which reports rather than resolves.** Removing
residency removes the receipts that made a wedged target visible, so the relay
emits inscriptions instead: a periodic aggregate of undelivered queue depth,
suppressed when the queue is empty, and a first-crossing warning per target once
its oldest `Pending` entry exceeds `[delivery].undelivered-warning-ms`. The
warning threshold is deduplicated **per target rather than per entry**, because
the condition an operator acts on is "this target is not draining", and a
per-entry rule would emit one line per queued message at the moment a wedged
target crosses. These are timers, and they pass the test for the same reason
residency fails it: elapsing causes a log line, and no member's outcome depends on
it.

### Decision 6 — At most one relay-authorized injection attempt

Not "at most once delivered" — the relay can only guarantee what it authorizes.
Transports do not deduplicate attempt IDs, so a stronger claim would be false.

**The relay never retries an authorized batch.** Retrying would require
coder-side idempotency that does not exist, and an autonomous agent acting twice
on a duplicated instruction is a correctness hazard, not an inconvenience. A
message that did not arrive, with an honest receipt, leaves the decision with the
sender — usually another agent, capable of asking.

Any member that reaches a terminal state SHALL do so **exactly once** in a
surviving relay process, including when a transport, worker, or collector panics.
That is uniqueness; completeness is a separate and weaker claim scoped in
*Decision 5* — authorized members are bounded, pending ones are not. A relay-owned
collector is **not sufficient** for uniqueness: `collect_outcome` today takes a
`JoinError` branch that releases the pending slot and returns without producing
any outcome — "a panic is a bug, not a delivery result"
(`src/relay/delivery/dispatch/outcomes.rs:80-96`) — and the per-target worker
task is itself unsupervised.

This requires a **relay-owned authorization guard**, owned outside every worker,
collector, and transport task. A keyed map plus a CAS is not enough on its own —
it cannot observe a detached Tmux or UI thread, a worker-task panic, a collector
panic, or a generation replacement. **Guard identity is created in two atomic steps**, because a packing unit does
not exist at authorization. A guard is created at authorization bound to
`(batch ID, member ID, attempt ID)`; when the transport
records its partition, each guard is **atomically bound to its `PackingUnit ID`**.
A pre-partition refusal or panic therefore terminalizes through the
batch/member-level guard without requiring a unit ID that was never assigned. An
earlier draft bound the guard to a unit ID at authorization, which was simply
impossible.

The guard then:

- normal evidence **consumes** the guard through one atomic non-terminal →
  terminal transition, so duplicate completions converge rather than racing;
- collectors carry keys rather than owning resolution;
- **unwind, channel closure, supervised task or thread exit, generation
  replacement, and graceful shutdown each terminalize the guard.** They are
  *triggers*; they do not choose the outcome. The guard applies one evidence
  order: the unit's immutable record if one exists, else `not_submitted` for a
  member never bound to a packing unit, else `submission_unknown`. Letting a
  lifecycle event pick the spelling would report `submission_unknown` for members
  the system can positively prove were never submitted;
- `Pending` entries are untouched by guard termination — they remain schedulable
  or take a pre-commit policy outcome.

**Generation fencing** is what makes this safe across respawn: a transport
generation is **torn down and fenced before its replacement begins**, so an old
generation cannot submit after its `Authorized` entries were resolved against it.
Without fencing, "resolved unknown" and "still able to act" coexist, which is the
target-side ordering hazard *Decision 9* has to reason about.

Fencing needs supervision the transports do not currently have. ACP moves its
client and child into a thread whose `JoinHandle` is **discarded**
(`src/acp/transport.rs:260-303`), respawn drops channels and can start a
replacement (`src/acp/worker_driver.rs:328-404`), and permission responders are
detached too. So the generation supervisor SHALL retain **every submission and
permission executor handle it owns**, plus the ability to invoke its transport's
generation termination primitive, and:

- **fence acknowledgment is a five-step state machine**: cooperative stop
  request, bounded cessation observation, non-blocking forced termination, second
  bounded observation, verdict. The spike settled the ordering — against an
  executor blocked in a primitive that observes no flag, observation alone never
  succeeds, and the forced termination is what lets it succeed. Two earlier
  drafts got this wrong: one stated the halves as an unordered conjunction, the
  other described it as terminate-then-*join*, which smuggled an unbounded wait
  back in;
- **each cessation observation is bounded** by
  `[delivery].fence-observation-timeout-ms`, and **neither may be a blocking
  join** — no runtime primitive can force a thread blocked in a syscall to
  return, so a join would reintroduce the unbounded wait this change removes.
  The termination primitive *initiates* and returns; step 4 observes. A
  successful invocation does not acknowledge the fence; observed cessation does;
- **the escalation is fixed**: invoke the transport's **generation termination
  primitive**, whose contract is to *initiate* cessation of every effect path the
  generation owns and return without blocking. Reaping a child is ACP's and Pty's
  implementation of it, not the universal action — UI owns no child, and Tmux
  reaches its target through a server it must not kill. The primitive unblocks an
  executor blocked writing into the terminated path, but it establishes nothing on
  its own: only the step-4 observation does, which is why a successful invocation
  never acknowledges the fence. A reap or a join MAY follow as cleanup once
  cessation has been observed; neither is the observation mechanism;
- **acknowledgment is bounded end to end**, not only before escalation. After
  the primitive is invoked the supervisor observes for cessation within a second
  window of the same configured duration, so the total is bounded by twice it.
  That observation must itself be bounded rather than a second blocking join,
  since no runtime primitive can force a thread blocked in a syscall to return;
- **if cessation is not positively observed within that window, the fence stays
  negative** and that target admits no replacement and releases no raw barrier.
  Timeout and failure both route here — there is no third outcome. Fail-stop is
  the right trade: a stuck target is operator-recoverable, an old generation
  writing alongside a new one is not;
- **terminalization does not require a *positive* verdict** — a negative verdict
  still terminalizes every still-unresolved member through the evidence order, so
  fail-stop strands no message. What a negative verdict withholds is the target's
  ordering barrier and its replacement, not the members' outcomes;
- **replacement and normal ordering barriers SHALL NOT proceed** until the fence
  is positive.

That split is the *Decision 9* three-facts distinction applied to respawn:
resolving an outcome and proving execution ceased are different events, and only
the second may release a barrier or admit a new generation.

The guard is what makes uniqueness a property of the system rather than an
aspiration about well-behaved tasks.

### Decision 7 — Outcomes are evidence, not position

The previous draft claimed post-commit outcomes never assert non-delivery, and
described committed-unconfirmed as "bytes sent". Both were wrong. Authorization
proves *responsibility transfer*, not that anything was written; and positive
evidence of non-submission is perfectly sound grounds for asserting non-delivery.

| Outcome | Wire spelling | Side | Evidence required |
|---|---|---|---|
| delivered | `delivered` | post | transport-specific **positive** evidence of injection |
| not submitted | `not_submitted` | post | positive evidence the unit produced no side effect |
| submission unknown | `submission_unknown` | post | none either way — panic, lost channel, partial write |
| dropped, transport unavailable | `transport_unavailable` | pre | positively observed terminal lifecycle; nothing authorized |
| dropped on shutdown | `dropped_on_shutdown` | pre | **still-pending relay-owned members only** |

**There is no `target failed` delivery outcome.** An earlier draft listed one,
which is unreachable under the observation window selected below: submission
success terminalizes `delivered` immediately, and before success the outcomes are
`not_submitted` or `submission_unknown`. A positively observed exit or close is
recorded as **target-health observability**, not as a delivery outcome — it
belongs to the readiness surface, not to any member's resolution. Keeping both
was a straightforward inconsistency.

Every non-delivered spelling above produces a terminal-outcome receipt, including
`not_submitted` and `submission_unknown`; both are also recorded per `Async
Delivery Observability`.

**`transport_unavailable` needs a policy boundary**, since "the transport is
gone" is not one condition. It fires only on a **positively observed terminal
lifecycle state** — the transport was shut down, or its generation was torn down
without replacement. A **transient absence** — a respawn in progress, a
generation being replaced, a UI subscriber that has disconnected but whose
session is still registered — leaves members `Pending`, until the absence
resolves into readiness, into a positively observed teardown, or into a sustained
unreachability the transport reports as `Unreachable` past the dwell. Nothing
converts the waiting itself into an outcome; otherwise `transport_unavailable`
would become another inference from absence, retired at the transport and
reintroduced at the relay.

**A readiness observation is not delivery evidence** on any transport that writes
after observing. Each transport's terminal evidence and its **observation window**
are specified, because a transport that can observe two facts in sequence would
otherwise resolve twice and violate *Decision 6*:

| Transport | `delivered` evidence | Window closes at |
|---|---|---|
| Tmux | `inject_literal_text` returns `Ok` | submission; a later pane death is target-health observability, not a delivery outcome |
| Pty | the unit's `write_all` pair to the master succeeds | submission; a later child exit is target-health observability |
| ACP | **write and flush of the complete newline-delimited `session/prompt` JSON-RPC request** succeeds (`src/acp/client.rs:114-121`, `:377-382`) | that framed write; the turn's later completion, permission requests, or connection close are target-health observability |
| UI | the broadcast is accepted by at least one live subscriber | submission |

ACP exposes no immediate protocol-level acceptance acknowledgment, so the framed
write is the strongest positive evidence available. Its evidence mapping is
therefore explicit: **active-prompt refusal and serialization failure are
`not_submitted`**; a stdin write or flush error **without proof that zero bytes
left** is `submission_unknown`. `Submitted` SHALL be recorded immediately after
the framed write succeeds — **before** replay-buffer locks or `on_dispatched`
(`:384-411`), either of which can block or panic and would otherwise strand
evidence that had already been earned.

**Submission success terminalizes `delivered` on every transport.** A later
positively observed exit or close is recorded as target health, not as a second
delivery outcome for an already-resolved member. This is the simpler of the two
options — the alternative, holding `delivered` open through a completion window
during which target-failure may still win, would make delivery outcomes depend on
how long the relay chose to watch, which is the class of judgement this change
exists to remove.

`not_submitted` is a **non-delivered** terminal outcome and produces a
terminal-outcome receipt exactly as `transport_unavailable` and
`dropped_on_shutdown` do.

`dropped_on_shutdown` applies only to members still pending at shutdown.
Authorized members resolve from evidence — `not submitted` where the transport
returned them unchanged, `submission unknown` otherwise.

A partially-succeeded write within one packing unit yields **submission unknown**
for that unit's members: the bytes may be on the target in truncated form, which
is neither delivery nor absence.

### Decision 8 — Crash recovery is scoped out, not resolved

The previous draft claimed a storage seam plus vocabulary "resolved" this. It
does not: an abrupt relay crash loses pending work, commit evidence, outcomes,
and sender notification alike, and no abstraction reconciles them after the fact.

**0.9.0 scope: guarantees hold for a surviving relay process and graceful
shutdown.** Stated as a limitation in the specs, not implied.

The seam constraints that keep durability reachable are still required now, since
retrofitting them is expensive:

- queue entries carry an explicit state — `Pending`, `Authorized`, `Terminal`
- each authorization carries a **stable attempt ID**

**Recovery behavior is specified only where it exists.** In-process recovery is
real and is specified now: when a per-target worker or transport is torn down and
respawned within a surviving relay, `Pending` entries are rescheduled to the new
generation, and `Authorized` entries are **never re-invoked** — they resolve
through the guard's evidence order. Respawn is a trigger for resolution, not a
chooser of outcomes: a unit that recorded `Submitted` still resolves `delivered`,
and a member never bound to a packing unit still resolves `not_submitted`. Process-startup recovery is **not**
specified, because nothing persists across a process boundary in 0.9.0; an
earlier draft claimed startup behavior was specified when no such behavior
exists, and that claim is withdrawn rather than restated.

Durable crash recovery is deferred with a follow-up issue.

### Decision 9 — Pty's write moves after its wait; `raww` keeps its ordering

Pty buffers then writes, as Tmux already does (`src/tmux/transport.rs:14-15`,
`:249`). The prompt-readiness template then actually gates injection instead of
merely deciding what the sender is told, and relay/62's absorption path ceases to
exist.

**`raww` bypasses readiness gating, not ordering.** It is currently a FIFO batch
barrier: a raw item "flushes any buffered envelope group first, then delivers as
its own write" (`src/transports/contract.rs:120-124`). That ordering is
preserved. The earlier phrase "immediate-commit bypass" was ambiguous between the
two meanings and is retired.

Preserving it takes a **relay-side scheduling rule**, and that rule turns on a
distinction an earlier draft collapsed. **Three facts are separate:**

| Fact | Established by | Releases |
|---|---|---|
| **Outcome terminal** | ledger CAS to a terminal spelling | admission quota, receipts, outcome-level barriers |
| **Execution ceased** | **positively observed cessation** within a bounded fence window | nothing on its own |
| **Target-side ordering safe** | execution ceased **and** no in-flight primitive can still take effect | the raw barrier |

`submission_unknown` is **terminal**. It resolves the member, releases quota, and
releases outcome-level barriers immediately — it is not a pending state. What it
does *not* establish is that the submission execution has stopped, which is
exactly why an earlier draft's "blocks raw until it resolves" was wrong: it had
already resolved.

So the ordering rule is:

- mail and raw are variants of one per-target relay FIFO; no authorization across
  a raw barrier, nor younger work across older;
- **raw waits for target-side ordering safety of older mail**, not merely for
  outcome terminality. A ledger transition to `submission_unknown` does not prove
  a still-running submission cannot take effect later.

**One raw mode.** Raw preserves FIFO and waits for target-side ordering safety.
There is no discriminator on the `raww` contract and no overtaking path.

#### Withdrawn: operator emergency raw

An earlier draft of this design specified a second mode — operator emergency raw,
on Tmux and Pty only, overtaking a target's `Pending` mail and bypassing the
readiness gate, selected through an explicit `mode` field on the `raww` contract,
the CLI `--mode` flag, and the MCP schema. It is **withdrawn from this change
entirely** on the operator's 2026-08-10 call, and filed as a standalone item at
`agentmux:todos/transports/8`. It is not deferred to this change's Phase 2.

Recorded because the reasoning is worth keeping and because the withdrawal
changed a claim this design previously made:

- **ACP was never a candidate, on protocol grounds rather than scheduling ones.**
  ACP raw is another `session/prompt`, and the client enforces one active prompt.
  Overtaking during an active turn yields a serialization refusal, not steering,
  and ACP has no byte-stream primitive to type past with. A tool-approval block —
  the case that most resembles needing to steer — resolves through the
  relay-injected `Chooser`, not through text. A challenge that the ACP exclusion
  was merely an artifact of raw and mail sharing one channel was raised in review
  and is wrong: Tmux and Pty share their channels too.
- **Overtaking an in-flight submission was never achievable on any transport.**
  On Pty, raw and envelope submission share one worker channel and one writer
  mutex; routing around it permits byte interleaving that corrupts both writes,
  and taking it blocks exactly as the normal path would. Tmux has the analogous
  constraint on its command path. Only overtaking *unauthorized* work was ever in
  scope, which is relay-side queue reordering.
- **Queue reordering is mechanically transport-agnostic but not uniformly
  useful.** It delivers steering only where the transport can accept the
  resulting raw handover, which is why the capability is Tmux and Pty rather than
  universal.

**The capability this removes, stated rather than discovered.** Pty raw today
interrupts an in-flight envelope readiness wait (`src/pty/delivery.rs:634-665`).
That mechanism cannot survive this change literally, because the wait it
interrupts is exactly what moves to relay `Pending`; keeping it would mean keeping
an in-transport readiness wait and reopening the liveness hole. The earlier draft
preserved the *capability* by substitution — relay-side emergency overtaking
standing in for the in-transport interrupt, both shipping in the core phase so no
window opened. With emergency raw withdrawn, the substitution is gone and the
capability lapses until `agentmux:todos/transports/8` lands.

This is an accepted regression, not an oversight: the operator confirmed the
capability is unused today. It is recorded here so that a future reader finds a
decision rather than an omission.

### Decision 10 — The contract applies to every transport, and there are five

Earlier drafts said "all three transports" throughout. `TransportImpl` is `Tmux`,
`Acp`, `Pty`, `Ui`, and the forward-declared `Pubsub` stub. Writing "three" was
the same error as omitting five requirements from a sweep last round: enumerating
from the set already in mind rather than from the type.

**UI is fully in scope** for relay-owned queueing, readiness, capacity,
authorization, and the ledger. It is not an edge case here — it is the
transport that most needs the change. Today it reports `Ready` unconditionally
(`src/transports/ui.rs:152-159`), spawns a thread per delivery, and resolves each
one through a **bounded reconnect wait** (`:180-188`). That wait is an
absence-adjudicating timer of exactly the kind this change retires: a subscriber
that has not reconnected within the window is not a subscriber that has failed.

Under this contract UI resolves like any other transport — `delivered` on a
broadcast accepted by at least one live subscriber, and `not submitted` when there
is positively no live subscriber. Mail for a registered UI session with no current
subscriber stays `Pending` until one attaches, because a closed browser tab is not
a failed delivery. The reconnect timeout is deleted, not relocated and not
replaced.

`Pubsub` is a stub with no delivery behavior, and "inherits the contract when it
gains one" is not a specification. Its behavior is stated concretely: a `Pubsub`
target is **rejected synchronously at admission** with the existing
not-implemented error, before anything is queued or authorized. It produces no
terminal outcome and no receipt, because nothing was ever accepted — which is why
it needs no new outcome spelling. **Work SHALL NOT be authorized merely to
discover the stub.** It is inside the uniform contract with a defined answer,
rather than silently outside it.

### Decision 11 — Health is a second axis, and unreachability is not unreadiness

*Decision 3* made readiness a single level: can this target take a handover right
now. Gating authorization on it exposed a case that level cannot express. A
target whose transport **cannot be observed at all** — a tmux session whose
server is gone, an ACP worker that has permanently failed — reports the same
`false` as a target that is merely busy. Under *Decision 5*'s unbounded `Pending`,
that means a member queued for a target that will never come back waits forever,
where it previously resolved from the failed write.

The defect is a collapse of two different findings into one bit:

| Finding | Meaning | Correct response |
|---|---|---|
| Observed, not ready | the target is busy, composing, or mid-turn | keep waiting; this is *Decision 5* working |
| Could not observe | the transport cannot reach its target at all | resolve the member; waiting learns nothing |

Only the first is evidence about *when* to hand over. The second is evidence
about *whether* handover is possible at all, and it is not a readiness question.

**Health is therefore a separate level, reported as a state rather than a bool:**

```rust
enum TransportHealth {
    Healthy,
    Unreachable { since: Instant },
}
```

A delivery attempt requires **both** axes: the transport SHALL be `Healthy` and
SHALL report `is_ready_for_handover`. Healthy-but-unready keeps the member
`Pending`, exactly as today. Unreachable resolves it.

**The `since` instant is the transport's; the threshold is the relay's.** The
transport reports what it observed and when it first observed it; the relay owns
the dwell policy as a `[delivery]` setting and decides what to do about it. Same
split as readiness — determination in the transport, scheduling in the relay —
and no back-edge.

**Sustained unreachability, not the first failed observation, is what bounces.**
A single probe that does not come back is not proof a target is gone: a fork
failure under load, a momentary hiccup, and a dead tmux server all present
identically at one observation. Bouncing on the first is trading a hang for a
false claim, which is worse, because a bounce asserts something to the sender
while a wait asserts nothing. A member is resolved only once its target has been
**continuously unreachable past the configured threshold**.

**This is not one of the timeouts being removed, and the distinction is exact.**
What the contract forbids is duration *substituting* for an observation — nothing
was seen, so after N seconds a verdict is guessed. That is why no bound converts
a readiness wait into an outcome: how long a target stays busy is not evidence
about the target. Here duration *qualifies* an observation that was actually
made, repeatedly. Sustained unreachability is itself evidence, in a way that
sustained busyness is not. Same clock, opposite epistemics.

The threshold's cost is stated rather than hidden: a target that recovers just
after it elapses will have had members bounced that could have been delivered.
That is inherent to any health check, and the threshold is the tuning point.

**Health gates writes and informs reads.** `raww` rides the same ordered channel
as delivery and inherits the gate. `look` SHALL NOT be blocked by it: an operator
looks at a target precisely when something is wrong with it, so refusing the
snapshot removes the diagnostic exactly when it is needed. `look` instead carries
the health level in its response metadata, which is strictly more informative
than an error — the last output that was captured, plus how long the target has
been unreachable.

**Transports are constructed when their worker starts, not on first write.**
Readiness and health are unanswerable for a transport that does not exist yet,
and the lazy construction they had to work around bought nothing: the worker is
spawned from a branch that already holds the task carrying the target member, so
nothing was deferred that could otherwise have been avoided. Every worker exists
because a task arrived for it. Construction moves to worker spawn, and a
construction failure resolves the task that triggered it.

**Where the bounce happens follows from that.** Admission runs on the request
path, before any worker exists, so admission cannot consult a transport that is
constructed at worker spawn. The bounce is therefore a **worker-side** resolution
one hop after admission, not an admission-time rejection. The sender observes the
same thing — a prompt terminal outcome rather than an indefinite wait — and the
only difference is that the queue briefly holds a member that is about to
resolve. Bouncing at admission would require a transport per configured target,
alive from relay startup; that is a coherent design and a larger one, and it is
not decided here.

## Rollout Ordering

The decisions above describe an end state. Reaching it across five transports
needs an order, and the phase line in `tasks.md` does not supply one: it is drawn
on quota-leak grounds — the guard cannot be deferred past `Authorized` — which
says what must be in the core, not what may land before a transport has migrated.

The relevant property is narrower than "does this transport implement the
contract yet". **There is exactly one boundary at which the new core and a
transport's existing prime/wedge/quiescence logic cannot coexist**, and it is
smaller than any transport's task list. Three tiers follow.

**Tier A — relay-local, coexistence-safe.** The queue entry state model,
admission and its quotas, per-target FIFO, the guard
and terminal CAS, typed submission evidence, and undelivered-queue reporting are
bookkeeping the relay performs over whatever the transport does. A transport that
still waits internally is not wrong under them; its entry simply holds `Authorized`
for the duration of that wait, which costs quota residency and nothing else. The
whole of Tier A may land before any transport changes, on one condition: the
`submission-timeout-ms` watchdog is not yet armed.

**Tier B — the cutover, and it must be simultaneous.** Arming the watchdog
requires that no transport waits for readiness inside its write seam. This is not
a sequencing preference; it is what the bound means. *Decision 5* and the
`runtime-bootstrap` capability both state that the bound covers ingestion and not
readiness, and they can only state it because authorization already implies
observed readiness. Arm it against a transport that still waits internally and it
fences healthy deliveries: Tmux's in-task quiescence wait is bounded by
`readiness_timeout_ms`, defaulting to fifteen minutes, and UI's reconnect wait to
thirty seconds, against a five-second watchdog. Pty fails the same boundary from
the other side — it writes before its prime wait, so it trips on evidence timing
rather than on readiness gating — and needs the same relocation.

No per-transport stub can defer this the way `mailw`'s additions-only default
deferred the write seam. `is_ready_for_handover` has no safe default: a default of
`true` authorizes a busy target straight into the watchdog, and a default of
`false` strands it permanently, since *Decision 5* leaves `Pending` unbounded. The
transport must answer for itself or not participate.

The lifecycle predicates that could have supplied a default are exactly the ones
*Decision 3* rejected, which is why the contract no longer carries one. Each
transport keeps its own privately where it still needs one — Pty gates its
`OutputView` on the runtime existing, a question `Busy` answers affirmatively and
handover readiness does not — so the wrong answer is no longer reachable through
the contract at all.

What must land simultaneously is only that: `is_ready_for_handover` plus deletion
of the internal readiness wait, on every transport, together with the watchdog. The
rest of each transport's work — evidence recording, partition placement, prime and
turn-timer deletion, ACP's staging queue — sequences independently.

**Tier C — fencing, coexistence-safe in the degraded direction only.** A
transport that cannot yet produce cessation evidence sits fence-negative, which
*Decision 6* makes safe by construction: no replacement is admitted, the raw
barrier holds, and every member still resolves through the guard. Safe is not the
same as recoverable. A negative fence is not a window that closes — for a
transport that can never observe cessation it is permanent, and blocked
replacement means that target can never be respawned. ACP is in exactly that
position while it discards its client/child handle (`agentmux:todos/relay/128`),
and the fence becomes reachable precisely when a target has wedged, which is when
respawn is the recovery path. **Retaining that handle therefore lands with Tier B,
not after it.**

Non-uniformity across transports inside one phase is already contemplated: `Ui`
carries its reconnect timer through Phase 1 by the interim exception in
`tasks.md`. The tiers above say which further non-uniformity is admissible and
which is not.

## Risks / Trade-offs

- **`submission_unknown` is pessimistic**, covering both "never started" and
  "wrote and died". → Accepted; the alternative is an acceptance protocol whose
  race is the complexity this design avoids. Revisit when a wire transport needs
  an acknowledgment.

- **This retires `bound-tmux-readiness-wait`, merged days ago.** → The bound
  addressed a real unbounded wait, and an earlier draft of this change concluded
  that what it got wrong was the *location* of the judgement, relocating the bound
  relay-side. That was still wrong: the wait should not be bounded at all, because
  waiting for a target to become ready is not a fault and a bound converts it into
  one. Nothing replaces it. The wait is unbounded by design and visible by
  inscription.

- **A permanently wedged target now holds its queue indefinitely**, where
  residency previously drained it. Its per-target quota fills, after which further
  sends to it are rejected at admission. → Accepted, and better than the
  alternative: the sender learns synchronously, at the request boundary, with a
  structured error it can act on, instead of learning nothing for the residency
  window and then receiving an `expired` receipt for a message that was silently
  discarded.

- **Enough wedged targets can exhaust relay-global quota** and begin rejecting
  sends to healthy targets; at the default `1_000` per target against `10_000`
  global, ten fully wedged targets suffice. Residency drained those queues over
  time and nothing else does. → Accepted rather than mitigated. A relay with ten
  permanently wedged targets is broken and should fail loudly at the request
  boundary; adding headroom reservation would buy partial availability in a
  situation that needs operator attention either way. The undelivered-queue
  inscriptions are what make it attributable before it reaches that point.

- **Per-unit outcomes are new work on Pty and ACP**, though Tmux already does it.
  → Required by *Decision 2*; Pty's single-outcome-per-group behavior is
  precisely the relay/62 defect.

- **At-most-once means some messages are silently not delivered** when a transport
  dies mid-flight. → Accepted per *Decision 6*; the sender is told, and the
  alternative risks an agent acting twice.

- **`src/pty/**` is behind the `pty` feature**, which no default gate builds and
  whose pre-commit clippy hook is file-scoped. → Default, `--features pty`, and
  ACP paths validated independently; this exact failure mode escaped four commits
  and three review rounds on the predecessor change.

- **Five config keys deleted across the three coder transports.** → The key list
  is settled here, but the `coders.toml` edits SHALL be made in the same window as
  the binary swap, not prepared ahead of it. There is no overlap: the running
  binary requires the keys, and the new one rejects them through
  `deny_unknown_fields`, which is the whole retirement mechanism. Editing early
  breaks the live relay; editing late fails the new binary's load. UI adds nothing
  to the list: its reconnect timeout is a constant plus builder
  (`src/transports/ui.rs:129-147`), not a TOML key, so retiring it is a code
  deletion rather than a config break.

- **The contract now reaches five transports, and UI's per-delivery threading and
  reconnect wait are a larger change than the coder transports'.** → In scope per
  *Decision 10*; carving UI out would leave an absence-adjudicating timer alive in
  the one transport whose "absence" is just a browser tab that closed.

## Open Questions

None blocking. Capacity units and crash-recovery scope were previously listed
here and are now settled in *Decisions 5 and 8*; carrying them as open questions
was itself a review finding. Residency was listed here as a placement question,
which framed it as "where does this bound live" and skipped the prior question of
whether it should exist. *Decision 5* answers that one instead.

Deliberately excluded, each a follow-up rather than a gap: durable crash
recovery; transport-side attempt-ID deduplication that would allow a stronger
guarantee than *Decision 6*; and mailbox-style delivery with acknowledged read
cursors (`agentmux:todos/runtime/23`), which is what restores the completeness
half of "resolves exactly once" without reintroducing a time-based expiry.
