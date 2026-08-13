## Context

Agentmux has three transport-local timers that each decide a delivery failed
because nothing changed: wedge detection, the prime timeout, and the readiness
bound. The live `Delivery Classifier` requirement already rejects that rule —
*"only a positively observed terminal event … is sound evidence of failure, and
an unchanged screen is not"* — four paragraphs above a timer that implements it.

Underneath them, **commitment is implicit, differs per transport, and is not even
a single event within one transport.** `mailw` is an enqueue, not a commit. Pty
writes every envelope to the pty master before its wait; ACP dispatches
`client.prompt(...)` and then starts its timer; only Tmux writes after its wait.
And a single flush group is split into **multiple independently fallible target
submissions** — Tmux partitions into token-budget prompts, ACP into envelope
groups via `batch_envelope_groups`, Pty into a `write_all` loop.

So "the delivery was committed" is not one fact. It is four: when the relay may
start, when it stops owning the message, when each target-side effect occurs, and
what any of that proves.

`agentmux:issues/relay/62` is the cost of leaving that implicit: a Pty flush
group's membership is mutable *after* its write, and `send_group_outcomes`
resolves every member identically — some committed, some not, all reported the
same. The red test that pinned this was deleted along with the transport-owned
wait, so the defect is currently unproven either way. Wedge detection masks it by
resolving groups in ~150 ms, so retiring the timers widens the window: the fix
and the retirement cannot be sequenced apart.

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

**Non-Goals:** crash recovery (*Decision 8*); durable queues, `fetch`/mailbox
retrieval (`agentmux:todos/runtime/23`), pub-sub fan-out; exactly-once delivery
(*Decision 6*); running transports as separate processes, though the boundary is
shaped for it.

## Decisions

### Decision 1 — Four events, one linearization point

| Event | Owner | Atomic? | Reversible? |
|---|---|---|---|
| **Admission** — accept into the queue, reserve capacity, return `queued` | relay | yes | yes (pre-commit) |
| **Authorization** — `Pending` → `Authorized` on the queue entry | relay | **yes — the linearization point** | **no** |
| **Submission** — one packing unit produces a target-side effect | transport | per unit | no |
| **Resolution** — each member resolves once, from recorded facts | transport → relay | per member | no |

After authorization the relay has irrevocably transferred delivery
responsibility: it never reclaims, never retries, and never asserts non-delivery
*by inference*. Positive evidence of non-submission remains reportable
(*Decision 7*).

Authorization is a relay-local state transition on the relay's own queue entry —
not a call, not a handshake, and not dependent on the transport observing
anything. That is why it is trivially atomic and why there is no acceptance race:
cancellation competes only with this transition, relay-locally.

**The invocation that follows is fallible, and no acceptance protocol is needed
to make it safe.** The tempting alternative — declare invocation non-rejectable
because the relay reserved capacity for it — is circular: the admission quota
reserves count and bytes *in the relay's own queue*, and says nothing about a
transport's channel, its live worker generation, or any target resource. `mailw`
legitimately fails today (Tmux resolves `channel_full` on a full or closed write
channel; ACP and Pty have equivalents). Since authorization has already ended
cancellation and retry, a post-authorization refusal is a *terminal evidence
result* rather than a reclaim: the transport returns the unit **unchanged** →
members resolve **not submitted**; side effects **cannot be excluded** → members
resolve **submission unknown**. The relay never reclaims either way, so there is
still no race to adjudicate.

Infallible acceptance, if ever genuinely required, needs
transport-generation-bound acceptance credits or reservation tokens. A count/byte
admission quota is not that mechanism.

#### All waiting happens before authorization

Retiring every absence timer removes the only thing that previously bounded a
stuck delivery. The two sides of authorization are bounded differently and
deliberately: a `Pending` member waits as long as its target takes, because
elapsed time spent waiting for readiness is not evidence about the target; an
`Authorized` member is bounded by `[delivery].submission-timeout-ms`, because
that clock measures our own supervised code rather than the target's behavior.

- **All target-readiness and quiescence waiting happens in `Pending`.** This says
  *when* the waiting happens, not who evaluates it: the relay owns the wait
  because it owns the queue; each transport owns the *determination* and reports
  it through `is_ready_for_handover`, since readiness does not generalise across
  a pane, a wire protocol, and a subscriber list (*Decision 3*).
- **Authorization starts exactly one immediate submission attempt**, and no
  `Authorized` batch may wait in a relay or transport staging queue. ACP's
  `mailw` today only `try_send`s into a transport channel, where a batch can sit
  behind an in-flight turn before partition — a post-authorization wait wearing a
  queue's clothing.
- **An older turn's completion and choices gate authorization of the next
  batch**; the newly authorized batch's own turn lifecycle does not hold its
  members' outcomes open. The causality forces this: ACP permission requests
  arrive only *because* the request was written, and completion arrives later
  still, so those events cannot precede the submission that causes them.
- **Any submission primitive that can block is supervised, fenced, and bounded**
  by `submission-timeout-ms` (operator confirmed mandatory, 2026-08-04). Every
  other trigger the guard has is an event, and an executor that stays alive and
  blocked produces none — so without this its quota leaks and its target's FIFO
  blocks forever.

On elapse the relay **initiates the generation fence and terminalizes nothing at
that moment**. Unit evidence stays admissible through both bounded fence windows,
and every still-unresolved member is terminalized through the guard's evidence
order at the verdict — the single resolution cut. Quota and outcome-level
barriers release with that terminalization; the target's FIFO, raw barrier, and
replacement release only on a **positive** verdict, so a negative one leaves that
target fail-stop while every other target keeps progressing.

The bound is an **execution watchdog over the relay's own supervised code**: it
states that our execution overran the time we allow it, not that the target
failed. That is what makes it categorically different from the timers being
retired, and why it yields `submission_unknown` rather than a failure spelling.

The split is where the bound belongs because of what each side holds. A `Pending`
member holds only its own queue slot, already reserved and bounded by admission.
An `Authorized` member holds its target's FIFO, an executor, and a transport
generation — so leaving that side unbounded stalls work unrelated to the stuck
member.

### Decision 2 — Batch versus packing unit

A **batch** is the unit of authorization. A **packing unit** is the unit of
target-side submission. They are not the same, and a batch is *not* one atomic
target write. Partition is fixed and exact before the first target-side effect;
no absorption across batches, which is the rule that makes relay/62 unreachable;
outcomes are per unit, so if unit 1 submits and unit 2 fails their members get
different outcomes.

**The invocation seam is the Batch, and partition is the transport's first
action.** This resolves what "a rejected invocation returns the unit unchanged"
meant when no unit yet existed: a refusal *before* partition returns the Batch
unchanged and terminalizes every member `not_submitted`, since nothing was
partitioned and nothing could have taken effect; a failure *after* partition
takes per-unit evidence. Pty members are singleton units, because Pty writes each
member with its own `write_all` pair.

**Identities are explicit and immutable.** A `Batch ID` and `Member ID` are
assigned at authorization, a `PackingUnit ID` at partition, and none is ever
reassigned; each authorization additionally carries a stable attempt ID
(*Decision 8*). Authorizing a batch authorizes every member atomically — there is
no partially-authorized batch. The full partition is retained for the batch's
lifetime, so resolution can attribute every member to the unit that carried it.

**Submission evidence is typed**, not inferred from an error string:

| Evidence | Meaning |
|---|---|
| `Submitted` | the target-side primitive positively reported success |
| `NotSubmitted` | positive evidence no side effect occurred |
| `SubmissionUnknown` | side effects cannot be excluded |

An undifferentiated error maps to `SubmissionUnknown`, never `NotSubmitted`. A
Tmux paste is a body write followed by Enter, and a Pty unit is multiple
`write_all` calls, so both can fail *after* partial effect. Only a primitive that
can prove nothing was written may report `NotSubmitted`.

**Unit evidence is recorded atomically before any member fan-out**, because
evidence is established per *unit* while guards terminalize per *member*. A
resolver that panicked halfway through fan-out would otherwise leave some members
`delivered` while the supervisor terminalized their siblings `submission_unknown`
— from identical target-side evidence. So: the unit's submission produces one
immutable evidence record; every member's outcome is derived from it; a panic
during fan-out resumes from the record rather than inventing an outcome for the
remainder. This is the difference between "one outcome per unit" as an intention
and as a property.

Queueing moves relay-side; rendering and packing stay transport-owned. The live
`Transport Module Boundaries` requirement does not distinguish queueing from
quiescence scheduling and packing, so it is sharpened rather than renamed.

### Decision 3 — Readiness is level-triggered and advisory

The relay needs to know when handing over is *useful*, not when it is *permitted*.

`is_ready_for_handover` is a level-triggered state and the transport contract's
only readiness predicate. The earlier `is_ready` is **removed rather than
redefined**: Tmux answered it unconditionally true, and ACP and Pty counted
`Busy` as ready. Two predicates were confusable exactly because "ready" does not
say what for; each transport keeps whatever lifecycle predicate it needs
privately.

The injected closure (*Decision 4*) delivers only an **edge**, so the relay must
subscribe-before-check, poll at a bounded cadence as a backstop, and re-read the
level on every notification, admission, and completion.

**If readiness changes between check and authorization, the invocation may fail**
— so stale readiness changes a delivery's *fate*, not merely its latency. That is
the honest statement once infallible acceptance is gone: readiness is a
scheduling hint whose staleness produces an evidence-based terminal outcome
rather than a lost or misreported message. It is still enough to avoid a lease,
token, or reject/ack protocol, because authorization — not acceptance — is the
linearization point.

### Decision 4 — No transport → relay back-edge

The relay calls transports; transports must not know relay interfaces. Where a
transport signals upward it invokes an opaque closure the relay **provided** at
construction — the pattern `PtyTransport` already uses for `mirror_state`, built
relay-side over `set_worker_readiness`.

Combined with *Decision 3*, correctness never depends on the back-edge: the
notification is an edge hint, the authoritative state is the level the relay
reads, and authorization is a relay-local transition. This keeps the boundary
one-directional and representable over a wire.

### Decision 5 — Capacity is contract; the relay does not bound its own patience

**Three quantities, previously conflated under "capacity", are separate:**

| Quantity | Owner | Purpose |
|---|---|---|
| **Admission quota** | relay | how much may be queued per target and relay-global; enforced at admission |
| **Maximum handover dimensions** | transport, static | the largest batch a transport will accept, in relay-evaluable units |
| **Acceptance capacity** | transport, dynamic | whether it can accept *right now*; surfaced as `is_ready_for_handover`, advisory per *Decision 1* |

All relay-facing quantities are expressed in **envelope count and canonical
payload bytes** — the serialized payload the relay already holds, not rendered
target text. Declaring them in tokens would be circular, since only the transport
can render and count those; `prompt_tokens_max` stays an internal packing-unit
limit governing *Decision 2*'s partition rather than batch size. **An envelope
exceeding the maximum handover dimensions on its own is rejected at admission
rather than queued unsendable** — which is what lets an empty handover window
always admit its first candidate, since an admitted envelope provably fits alone.

Capacity is atomically reserved at admission before `queued` is returned, and
released at the member's terminal transition through the guard (*Decision 6*), so
a panicked task cannot leak it. Scheduling is FIFO per target, where FIFO means
worker-enqueue linearization — a request may reserve admission before another and
still lose the enqueue race, so admission order is not delivery order and only
the latter is guaranteed. The policy lives in relay configuration, not
`coders.toml`, because it is a property of the relay's own queue.

**Across targets, nothing is scheduled.** Byte-budgeted round-robin with a
configured quantum was specified and withdrawn: the quantum had to be at least
the largest permitted handover byte component while batch formation was already
capped at that same component, so one quantum always afforded a full batch and
the credit could bind only on a second batch within a visit — and visits existed
only because a rotation did. The budget was there to be fair within a rotation;
the rotation was there to allocate the budget.

Targets *do* contend — Tmux targets in a bundle share one server and socket, ACP
bootstrap enters a shared blocking pool, a blocking write seam occupies a
delivery-runtime worker thread — but a global byte quantum measures none of them.
A resource-grounded policy would be denominated per shared resource and would owe
a throughput or fairness objective that nothing requires today. And a transport
blocking the write seam violates the non-blocking handover contract: a defect to
repair at its source, not a load to schedule around.

**There is no residency bound and no `expired` outcome.** Ask what elapsing
*proves*. For `submission-timeout-ms` it proves our own supervised code overran
its allowance — directly observed, reported as `submission_unknown`. For
residency it proves only that a target has not become ready yet, and is used to
conclude a message should not be delivered: inference from absence, retired at
the transport and reintroduced at the relay under a calmer name.

Expiry is not a report. It terminalizes the member and releases its quota, so a
message that would have landed when the agent finished a long turn does not land.
**We would be dropping mail to keep a guarantee sentence true** — and the
sentence cannot be honestly kept, because any bound we pick is a guess about
someone else's work, and every message it expires is a message the guess got
wrong.

Residency's apparent jobs have owners that do not require it:

| Apparent job | Real owner | Basis |
|---|---|---|
| Bound queue memory | admission quota, count and bytes | positive accounting, enforced before `queued` returns |
| Resolve mail to a dead target | `transport_unavailable` | positively observed terminal lifecycle |
| Unstick a live but blocked target | operator teardown | positive action |

The fourth job is the guarantee, and "resolves exactly once" is three claims
wearing one sentence. **Uniqueness** — no member produces two terminal outcomes —
is the terminal CAS of *Decision 6*, untouched. **Bounded completeness for
`Authorized` members** — within `submission-timeout-ms` plus twice the fence
observation budget, on positive and negative verdicts alike — is untouched.
**Completeness for `Pending` members** is the one residency propped up, and it is
dropped. Collapsing the three is what made residency look load-bearing.

The honest replacement: **every accepted message resolves at most once, on
delivery, on submission evidence, on a positively observed terminal target
lifecycle, on sustained unreachability past the dwell, or at relay shutdown.** A
message queued for a live target that never becomes ready stays `Pending`
indefinitely, holding no caller and no thread — sends are asynchronous by
contract — only a queue slot that admission already bounded. Restoring
completeness is a mailbox problem, not a timer problem
(`agentmux:todos/runtime/23`).

**Compensating observability reports rather than resolves.** The relay emits a
periodic aggregate of undelivered queue depth, suppressed when the queue is
empty, and a first-crossing warning per target once its oldest `Pending` entry
exceeds `[delivery].undelivered-warning-ms`. Deduplicated **per target rather
than per entry**, because the condition an operator acts on is "this target is
not draining". These are timers, and they pass the test residency fails for the
same reason: elapsing causes a log line, and no member's outcome depends on it.

### Decision 6 — At most one relay-authorized injection attempt

Not "at most once delivered" — the relay can only guarantee what it authorizes,
and transports do not deduplicate attempt IDs.

**The relay never retries an authorized batch.** Retrying would require
coder-side idempotency that does not exist, and an autonomous agent acting twice
on a duplicated instruction is a correctness hazard, not an inconvenience. A
message that did not arrive, with an honest receipt, leaves the decision with the
sender — usually another agent, capable of asking.

Uniqueness holds **including when a transport, worker, or collector panics**. A
relay-owned collector is not sufficient: `collect_outcome` today takes a
`JoinError` branch that releases the pending slot and returns without producing
any outcome, and the per-target worker task is itself unsupervised. So the design
requires a **relay-owned authorization guard**, owned outside every worker,
collector, and transport task — a keyed map plus CAS cannot observe a detached
thread, a worker-task panic, or a generation replacement.

**Guard identity is created in two atomic steps**, because a packing unit does
not exist at authorization: a guard is created bound to `(batch ID, member ID,
attempt ID)`, then atomically bound to its `PackingUnit ID` when the transport
records its partition. A pre-partition refusal or panic therefore terminalizes
without requiring a unit ID that was never assigned.

The guard then: consumes normal evidence through one atomic non-terminal →
terminal transition, so duplicate completions converge rather than race; leaves
collectors carrying keys rather than owning resolution; and treats **unwind,
channel closure, supervised task or thread exit, generation replacement, and
graceful shutdown as *triggers* that do not choose the outcome**. One evidence
order applies: the unit's immutable record if one exists, else `not_submitted`
for a member never bound to a packing unit, else `submission_unknown`. Letting a
lifecycle event pick the spelling would report `submission_unknown` for members
the system can positively prove were never submitted. `Pending` entries are
untouched by guard termination.

**Generation fencing** makes this safe across respawn: a generation is torn down
and fenced before its replacement begins, so an old generation cannot submit
after its `Authorized` entries were resolved against it. Without it, "resolved
unknown" and "still able to act" coexist — the target-side ordering hazard
*Decision 9* reasons about.

Fencing needs supervision the transports lack today (ACP discards the
`JoinHandle` for its client and child; permission responders are detached). The
generation supervisor retains every submission and permission executor handle it
owns, plus the ability to invoke its transport's generation termination
primitive, and:

- **Fence acknowledgment is a five-step state machine**: cooperative stop
  request, bounded cessation observation, non-blocking forced termination, second
  bounded observation, verdict. The ordering is load-bearing — against an
  executor blocked in a primitive that observes no flag, observation alone never
  succeeds, and the forced termination is what lets it succeed.
- **Each observation is bounded** by `[delivery].fence-observation-timeout-ms`
  and **neither may be a blocking join**: no runtime primitive can force a thread
  blocked in a syscall to return, so a join would reintroduce the unbounded wait
  this change removes. Acknowledgment is therefore bounded end to end by twice
  that duration.
- **The escalation is fixed**: invoke the transport's generation termination
  primitive, whose contract is to *initiate* cessation of every effect path the
  generation owns and return without blocking. Reaping a child is ACP's and Pty's
  implementation of it, not the universal action — UI owns no child, and Tmux
  reaches its target through a server it must not kill. A successful invocation
  never acknowledges the fence; only the step-4 observation does. A reap or a
  join MAY follow as cleanup once cessation has been observed; neither is the
  observation mechanism.
- **If cessation is not positively observed, the fence stays negative**: that
  target admits no replacement and releases no raw barrier. Timeout and failure
  both route here; there is no third outcome. A stuck target is
  operator-recoverable, an old generation writing alongside a new one is not.
- **A negative verdict still terminalizes** every unresolved member through the
  evidence order, so fail-stop strands no message. What it withholds is the
  target's ordering barrier and its replacement, not the members' outcomes.

That split is *Decision 9*'s three-facts distinction applied to respawn: resolving
an outcome and proving execution ceased are different events, and only the second
may release a barrier or admit a new generation.

### Decision 7 — Outcomes are evidence, not position

Authorization proves *responsibility transfer*, not that anything was written;
and positive evidence of non-submission is sound grounds for asserting
non-delivery.

| Outcome | Wire spelling | Side | Evidence required |
|---|---|---|---|
| delivered | `delivered` | post | transport-specific **positive** evidence of injection |
| not submitted | `not_submitted` | post | positive evidence the unit produced no side effect |
| submission unknown | `submission_unknown` | post | none either way — panic, lost channel, partial write |
| dropped, transport unavailable | `transport_unavailable` | pre | positively observed terminal lifecycle; nothing authorized |
| dropped on shutdown | `dropped_on_shutdown` | pre | **still-pending relay-owned members only** |

**There is no `target failed` delivery outcome.** It is unreachable under the
observation window below: submission success terminalizes `delivered`
immediately, and before success the outcomes are `not_submitted` or
`submission_unknown`. A positively observed exit or close is **target-health
observability**, belonging to the readiness surface rather than to any member's
resolution.

**`transport_unavailable` needs a policy boundary**, since "the transport is
gone" is not one condition. It fires only on a positively observed terminal
lifecycle state. A transient absence — a respawn in progress, a generation being
replaced, a UI subscriber disconnected but still registered — leaves members
`Pending` until the absence resolves into readiness, into observed teardown, or
into sustained unreachability past the dwell. Nothing converts the waiting itself
into an outcome; otherwise this becomes another inference from absence.

**A readiness observation is not delivery evidence** on any transport that writes
after observing. Each transport's evidence and **observation window** are
specified, because a transport that can observe two facts in sequence would
otherwise resolve twice:

| Transport | `delivered` evidence | Window closes at |
|---|---|---|
| Tmux | `inject_literal_text` returns `Ok` | submission; later pane death is target health |
| Pty | the unit's `write_all` pair to the master succeeds | submission; later child exit is target health |
| ACP | write and flush of the complete newline-delimited `session/prompt` request succeeds | that framed write; later completion, permission requests, or close are target health |
| UI | the broadcast is accepted by at least one live subscriber | submission |

ACP exposes no immediate protocol-level acknowledgment, so the framed write is
the strongest positive evidence available: active-prompt refusal and
serialization failure are `not_submitted`; a stdin write or flush error without
proof that zero bytes left is `submission_unknown`. `Submitted` is recorded
immediately after the framed write succeeds — **before** replay-buffer locks or
`on_dispatched`, either of which can block or panic and would strand evidence
already earned.

**Submission success terminalizes `delivered` on every transport.** The
alternative — holding `delivered` open through a completion window during which
target-failure may still win — would make delivery outcomes depend on how long
the relay chose to watch, which is the class of judgement this change exists to
remove.

Every non-delivered spelling produces a terminal-outcome receipt, including
`not_submitted` and `submission_unknown`, and both are recorded per `Async
Delivery Observability`. `dropped_on_shutdown` applies only to members still
pending at shutdown; authorized members resolve from evidence — `not submitted`
where the transport returned them unchanged, `submission unknown` otherwise.

A partially-succeeded write within one packing unit yields `submission_unknown`:
the bytes may be on the target in truncated form, which is neither delivery nor
absence.

### Decision 8 — Crash recovery is scoped out, not resolved

An abrupt relay crash loses pending work, commit evidence, outcomes, and sender
notification alike, and no storage abstraction reconciles them after the fact.
**0.9.0 scope: guarantees hold for a surviving relay process and graceful
shutdown**, stated as a limitation in the specs rather than implied.

Two seam constraints keep durability reachable, since retrofitting them is
expensive: queue entries carry an explicit state (`Pending`, `Authorized`,
`Terminal`), and each authorization carries a stable attempt ID.

**Recovery behavior is specified only where it exists.** In-process recovery is
real: when a per-target worker or transport is torn down and respawned within a
surviving relay, `Pending` entries are rescheduled to the new generation and
`Authorized` entries are **never re-invoked** — they resolve through the guard's
evidence order. Respawn is a trigger for resolution, not a chooser of outcomes.
Process-startup recovery is not specified, because nothing persists across a
process boundary in 0.9.0.

### Decision 9 — Pty's write moves after its wait; `raww` keeps its ordering

Pty buffers then writes, as Tmux already does. The prompt-readiness template then
actually gates injection instead of merely deciding what the sender is told, and
relay/62's absorption path ceases to exist.

**`raww` bypasses readiness gating, not ordering.** It is a FIFO batch barrier: a
raw item flushes any buffered envelope group first, then delivers as its own
write. Preserving that takes a relay-side scheduling rule, which turns on three
separate facts:

| Fact | Established by | Releases |
|---|---|---|
| **Outcome terminal** | ledger CAS to a terminal spelling | admission quota, receipts, outcome-level barriers |
| **Execution ceased** | positively observed cessation within a bounded fence window | nothing on its own |
| **Target-side ordering safe** | execution ceased **and** no in-flight primitive can still take effect | the raw barrier |

`submission_unknown` is **terminal** — it resolves the member and releases quota
immediately. What it does *not* establish is that the submission execution has
stopped. So: mail and raw are variants of one per-target relay FIFO, with no
authorization across a raw barrier nor younger work across older; and **raw waits
for target-side ordering safety of older mail**, not merely for outcome
terminality.

**One raw mode.** There is no discriminator on the `raww` contract and no
overtaking path. Operator emergency raw — a second mode overtaking a target's
`Pending` mail on Tmux and Pty — is withdrawn from this change entirely
(operator, 2026-08-10) and filed as `agentmux:todos/transports/8`. Two findings
from that analysis outlive it: ACP was never a candidate on protocol grounds,
since ACP raw is another `session/prompt` and the client enforces one active
prompt — and a tool-approval block, the case that most resembles needing to
steer, resolves through the relay-injected `Chooser` rather than through text.
Overtaking an *in-flight* submission was never achievable on any
transport, because raw and envelope submission share one worker channel and one
writer mutex, so only overtaking *unauthorized* work — relay-side queue
reordering — was ever in scope. That reordering is mechanically
transport-agnostic but not uniformly useful: it delivers steering only where the
transport can accept the resulting raw handover, which is why the capability was
scoped to Tmux and Pty rather than made universal.

**An accepted regression, recorded so a future reader finds a decision rather
than an omission:** Pty raw today interrupts an in-flight envelope readiness
wait. That mechanism cannot survive this change, because the wait it interrupts
is exactly what moves to relay `Pending`. The capability lapses until
`agentmux:todos/transports/8` lands; the operator confirmed it is unused today.

### Decision 10 — The contract applies to every transport, and there are five

`TransportImpl` is `Tmux`, `Acp`, `Pty`, `Ui`, and the forward-declared `Pubsub`
stub.

**UI is fully in scope** for relay-owned queueing, readiness, capacity,
authorization, and the ledger — it is the transport that most needs the change.
Today it reports `Ready` unconditionally, spawns a thread per delivery, and
resolves each through a bounded reconnect wait: an absence-adjudicating timer of
exactly the kind this change retires, since a subscriber that has not reconnected
within the window is not a subscriber that has failed. Under this contract UI
resolves `delivered` on a broadcast accepted by at least one live subscriber and
`not submitted` when there is positively no live subscriber; mail for a
registered UI session with no current subscriber stays `Pending` until one
attaches, because a closed browser tab is not a failed delivery. The reconnect
timeout is deleted, not relocated.

**`Pubsub` is given a concrete answer rather than an inheritance clause:** a
`Pubsub` target is rejected synchronously at admission with the existing
not-implemented error, before anything is queued or authorized. It produces no
terminal outcome and no receipt, because nothing was accepted — which is why it
needs no new outcome spelling. Work is not authorized merely to discover the
stub.

### Decision 11 — Health is a second axis, and unreachability is not unreadiness

*Decision 3* made readiness a single level: can this target take a handover right
now. A target whose transport **cannot be observed at all** — a tmux session
whose server is gone, an ACP worker that has permanently failed — reports the
same `false` as a target that is merely busy. Under *Decision 5*'s unbounded
`Pending`, a member queued for a target that will never come back would wait
forever, where it previously resolved from the failed write.

| Finding | Meaning | Correct response |
|---|---|---|
| Observed, not ready | the target is busy, composing, or mid-turn | keep waiting; this is *Decision 5* working |
| Could not observe | the transport cannot reach its target at all | resolve the member; waiting learns nothing |

Only the first is evidence about *when* to hand over. The second is evidence
about *whether* handover is possible at all, and it is not a readiness question.
So health is a separate level, reported as a state rather than a bool
(`Healthy` / `Unreachable { since }`), and a delivery attempt requires **both**
axes.

**The `since` instant is the transport's; the threshold is the relay's** — same
split as readiness, determination in the transport and scheduling in the relay,
and no back-edge.

**Sustained unreachability, not the first failed observation, is what bounces.**
A fork failure under load, a momentary hiccup, and a dead tmux server all present
identically at one observation. Bouncing on the first trades a hang for a false
claim, which is worse: a bounce asserts something to the sender while a wait
asserts nothing. A member is resolved only once its target has been
**continuously** unreachable past the configured threshold, so a target that
recovers and lapses again starts the dwell over.

**This is not one of the timeouts being removed, and the distinction is exact.**
What the contract forbids is duration *substituting* for an observation — nothing
was seen, so after N seconds a verdict is guessed. Here duration *qualifies* an
observation that was actually made, repeatedly. Sustained unreachability is
itself evidence in a way that sustained busyness is not. Same clock, opposite
epistemics. The cost is stated rather than hidden: a target that recovers just
after the threshold elapses will have had members bounced that could have been
delivered.

**Health gates writes and does not gate reads.** `raww` rides the same ordered
channel and inherits the gate. A `look` is never rejected on account of its
target's health: an operator looks at a target precisely when something is wrong
with it. What comes back is then the transport's own business, and this design
promises no more — a tmux transport reports `Unreachable` *because* its pane
cannot be observed, and that is the same pane a snapshot would come from, so the
attempt fails on its own terms. The diagnostic difference that survives is
narrower than "the operator still sees the pane": it is that the operator
receives the transport's real error rather than a policy refusal. Carrying the
health level in the response would be strictly more informative than either and
is tracked as `agentmux:issues/relay/67`, not assumed here.

**Transports are constructed when their worker starts, not on first write**,
because readiness and health are unanswerable for a transport that does not exist
yet. Every worker exists because a task arrived for it, so nothing was deferred
that could otherwise have been avoided.

**The bounce is therefore worker-side**, one hop after admission, rather than an
admission-time rejection: admission runs on the request path, before any worker
exists. The sender observes the same thing — a prompt terminal outcome rather
than an indefinite wait. Bouncing at admission would require a transport per
configured target, alive from relay startup; a coherent design, a larger one, and
not decided here.

## Rollout Ordering

The decisions above describe an end state; reaching it across five transports
needs an order. The phase line in `tasks.md` is drawn on quota-leak grounds — the
guard cannot be deferred past `Authorized` — which says what must be in the core,
not what may land before a transport has migrated. **There is exactly one
boundary at which the new core and a transport's existing prime/wedge/quiescence
logic cannot coexist**, and it is smaller than any transport's task list.

**Tier A — relay-local, coexistence-safe.** The queue entry state model,
admission and its quotas, per-target FIFO, the guard and terminal CAS, typed
submission evidence, and undelivered-queue reporting are bookkeeping the relay
performs over whatever the transport does. A transport that still waits
internally is not wrong under them; its entry holds `Authorized` for the duration
of that wait, costing quota residency and nothing else. All of Tier A may land
before any transport changes, on one condition: the `submission-timeout-ms`
watchdog is not yet armed.

**Tier B — the cutover, and it must be simultaneous.** Arming the watchdog
requires that no transport waits for readiness inside its write seam. This is
what the bound *means*: *Decision 5* and the `runtime-bootstrap` capability both
state that it covers ingestion and not readiness, and can only state it because
authorization already implies observed readiness. Arm it against a transport that
still waits internally and it fences healthy deliveries — Tmux's in-task
quiescence wait is bounded by `readiness_timeout_ms`, defaulting to fifteen
minutes, and UI's reconnect wait to thirty seconds, against a five-second
watchdog. Pty fails the same boundary from
the other side, writing before its prime wait.

No per-transport stub can defer this. `is_ready_for_handover` has no safe
default: `true` authorizes a busy target straight into the watchdog, and `false`
strands it permanently, since `Pending` is unbounded. The transport must answer
for itself or not participate. The lifecycle predicates that could have supplied
a default are the ones *Decision 3* rejected — Pty gates its `OutputView` on the
runtime existing, a question `Busy` answers affirmatively and handover readiness
does not — which is why the wrong answer is no longer reachable through the
contract at all.

What must land simultaneously is only that: `is_ready_for_handover` plus deletion
of the internal readiness wait, on every transport, together with the watchdog.
Each transport's remaining work sequences independently.

**Tier C — fencing, coexistence-safe in the degraded direction only.** A
transport that cannot yet produce cessation evidence sits fence-negative, which
*Decision 6* makes safe by construction. Safe is not recoverable: a negative
fence is not a window that closes, and blocked replacement means that target can
never be respawned. ACP is in exactly that position while it discards its
client/child handle (`agentmux:todos/relay/128`), and the fence becomes reachable
precisely when a target has wedged — which is when respawn is the recovery path.
**Retaining that handle therefore lands with Tier B, not after it.**

## Risks / Trade-offs

- **`submission_unknown` is pessimistic**, covering both "never started" and
  "wrote and died". → Accepted; the alternative is an acceptance protocol whose
  race is the complexity this design avoids.
- **This retires `bound-tmux-readiness-wait`, merged days ago.** → The wait
  should not be bounded at all: waiting for a target to become ready is not a
  fault, and a bound converts it into one. Nothing replaces it; the wait is
  unbounded by design and visible by inscription.
- **A permanently wedged target holds its queue indefinitely**, where residency
  drained it; its per-target quota fills and further sends are rejected at
  admission. → Accepted, and better: the sender learns synchronously at the
  request boundary with a structured error, instead of learning nothing for the
  residency window and then receiving an `expired` receipt for a message that was
  silently discarded.
- **Enough wedged targets can exhaust relay-global quota** — at the default
  `1_000` per target against `10_000` global, ten suffice — and begin rejecting
  sends to healthy targets. → Accepted rather than mitigated. A relay with ten
  permanently wedged targets is broken and should fail loudly; headroom
  reservation would buy partial availability in a situation needing operator
  attention either way. The undelivered-queue inscriptions make it attributable
  first.
- **Per-unit outcomes are new work on Pty and ACP.** → Required by *Decision 2*;
  Pty's single-outcome-per-group behavior is precisely the relay/62 defect.
- **At-most-once means some messages are silently not delivered** when a
  transport dies mid-flight. → Accepted per *Decision 6*; the sender is told, and
  the alternative risks an agent acting twice.
- **`src/pty/**` is behind the `pty` feature**, which no default gate builds and
  whose pre-commit clippy hook is file-scoped. → Default, `--features pty`, and
  ACP paths validated independently; this exact failure mode escaped four commits
  and three review rounds on the predecessor change.
- **Five config keys deleted across the three coder transports.** → The
  `coders.toml` edits are made in the same window as the binary swap, not ahead
  of it: the running binary requires the keys and the new one rejects them
  through `deny_unknown_fields`, so editing early breaks the live relay and
  editing late fails the new binary's load. UI adds nothing to the list — its
  reconnect timeout is a constant plus builder, not a TOML key.
- **UI's per-delivery threading and reconnect wait are a larger change than the
  coder transports'.** → In scope per *Decision 10*; carving UI out would leave
  an absence-adjudicating timer alive in the one transport whose "absence" is
  just a browser tab that closed.

## Open Questions

None blocking. Deliberately excluded, each a follow-up rather than a gap: durable
crash recovery; transport-side attempt-ID deduplication that would allow a
stronger guarantee than *Decision 6*; and mailbox-style delivery with
acknowledged read cursors (`agentmux:todos/runtime/23`), which is what restores
the completeness half of "resolves exactly once" without reintroducing a
time-based expiry.
