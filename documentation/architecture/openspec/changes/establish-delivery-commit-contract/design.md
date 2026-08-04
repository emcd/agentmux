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
reported the same. Red test at `c96a45e`. Wedge detection masks it by resolving
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
- Every accepted message resolves exactly once in a surviving relay process,
  including when a transport panics.
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
reserved capacity for it. That reasoning was circular: relay residency reserves
count and bytes **in the relay's own queue**, and reserves nothing about a
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
stuck delivery. Nothing replaces it for an `Authorized` member: residency governs
`Pending` only, and a ledger records state without manufacturing a terminal event
while the owner is alive and blocked. An earlier draft left Pty buffering,
waiting, then writing *after* authorization, which would have made an authorized
member hang indefinitely with no bound anywhere in the system.

So the rule is structural, not incidental:

- **All target-readiness and quiescence waiting happens in `Pending`**, before
  authorization: prompt-readiness matching, quiescence observation, and — on ACP
  — completion and operator-choice resolution of an **older** turn.
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
  fenced/interruptible. An *operational* bound on that primitive may terminalize
  `SubmissionUnknown`, which is sound because it states our own execution
  evidence rather than a claim about target health — the distinction that makes
  it categorically different from the timers being retired.

This is why residency is meaningful: the members that wait are exactly the ones
for which nothing has been submitted, so expiring them asserts non-delivery on
solid ground.

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

- Add a **level-triggered** `can_accept_handover` capacity/readiness state.
  Existing surfaces cannot serve: Tmux `is_ready` is always true, and ACP and Pty
  count `Busy` as ready.
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

### Decision 5 — Capacity and residency are contract, not deferred detail

Both were previously filed as open questions. They are load-bearing and are
settled here.

**Three distinct quantities were previously conflated under "capacity" and are
now separate:**

| Quantity | Owner | Purpose |
|---|---|---|
| **Admission quota** | relay | how much may be queued per target and relay-global; enforced at admission |
| **Maximum handover dimensions** | transport, static | the largest batch a transport will accept, in relay-evaluable units |
| **Acceptance capacity** | transport, dynamic | whether it can accept *right now*; surfaced as `can_accept_handover`, and **advisory** — a stale reading yields a fallible invocation per *Decision 1*, not a guarantee |

All relay-facing quantities are expressed in units the relay can evaluate without
packing: **envelope count and canonical payload bytes**, where canonical bytes
means the serialized envelope payload the relay already holds, not rendered
target text. Declaring them in tokens would be circular, since only the transport
can render and count those. `prompt_tokens_max` remains an **internal
packing-unit limit**, invisible to the relay, governing *Decision 2*'s partition
rather than the batch size. An envelope exceeding the maximum handover dimensions
on its own is rejected at admission rather than queued unsendable.

**Residency and size policy:**

- Per-target and relay-global bounds, both enforced.
- Capacity is **atomically reserved at admission, before `queued` is returned**,
  and released at the member's terminal transition — whether that is pre-commit
  expiry or post-commit resolution.
- Scheduling is FIFO per target. Across targets it is **byte-budgeted
  round-robin**, specified rather than named:
  - **cost unit** — canonical payload bytes, the same unit as admission quota, so
    one accounting serves both;
  - **quantum** — a relay-configured byte value, which SHALL be **greater than or
    equal to the canonical-payload-byte component of every registered transport's
    maximum handover dimensions**. Configuring it lower is a validation error at
    load;
  - **per-visit credit** — exactly one quantum, with no carry-over. One spend
    limit, not two;
  - **oversized item** — cannot exist. Because the quantum is at least the
    largest permitted byte component, and admission already rejects an envelope
    exceeding the transport's maximum handover dimensions, every admissible item
    fits within one quantum;
  - **eligible rotation** — only targets with pending work and a transport
    reporting `can_accept_handover` are visited; ineligible targets are skipped.

  **A deficit counter was specified here and has been removed.** Classical DRR
  accumulates unspent quantum so an item larger than one quantum can eventually
  be sent, and the oversized-item rule above excludes that case by construction.
  The counter therefore had no work to do, while the anti-monopoly cap it needed
  reduced available credit back to one quantum anyway. Carrying both a carry-over
  rule and a cap produced two incompatible spend limits — a visit could claim
  "remaining quantum plus deficit", up to two quanta, while an acceptance
  scenario forbade exceeding one. Removing the counter leaves a single limit and
  slightly stronger fairness: every eligible target is visited each rotation and
  receives a full quantum.

  This replaces an earlier "no target may be starved", which named a property
  without defining a mechanism that could be tested for it.
- Admission quota is released through the guard's terminal transition
  (*Decision 6*) rather than by the collector, so a panicked task cannot leak it.
- The policy lives in relay configuration, not `coders.toml`, because it is a
  property of the relay's patience rather than of any coder. Per-target overrides
  are deliberately excluded from this change; a UI session and a long-horizon
  coder plausibly warrant different patience, and that is a follow-up.

Residency expiry is a **pre-commit** outcome only. It is a statement about the
relay's own patience, never about the target's health, and it cannot fire once a
message is authorized.

### Decision 6 — At most one relay-authorized injection attempt

Not "at most once delivered" — the relay can only guarantee what it authorizes.
Transports do not deduplicate attempt IDs, so a stronger claim would be false.

**The relay never retries an authorized batch.** Retrying would require
coder-side idempotency that does not exist, and an autonomous agent acting twice
on a duplicated instruction is a correctness hazard, not an inconvenience. A
message that did not arrive, with an honest receipt, leaves the decision with the
sender — usually another agent, capable of asking.

Every accepted member SHALL resolve **exactly once** in a surviving relay
process, including when a transport, worker, or collector panics. A relay-owned
collector is **not sufficient** for this: `collect_outcome` today takes a
`JoinError` branch that releases the pending slot and returns without producing
any outcome — "a panic is a bug, not a delivery result"
(`src/relay/delivery/dispatch/outcomes.rs:80-96`) — and the per-target worker
task is itself unsupervised.

This requires a **relay-owned authorization guard**, owned outside every worker,
collector, and transport task. A keyed map plus a CAS is not enough on its own —
it cannot observe a detached Tmux or UI thread, a worker-task panic, a collector
panic, or a generation replacement. **Guard identity is created in two atomic steps**, because a packing unit does
not exist at authorization. A guard is created at authorization bound to
`(batch ID, member ID, attempt ID, transport generation)`; when the transport
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
detached too. So the generation supervisor SHALL retain **child termination
authority plus every submission and permission executor handle** it owns, and:

- **fence acknowledgment is ordered**: terminate first, then join every
  generation-owned executor. The spike settled this — against an executor blocked
  in a primitive that observes no flag, a join alone never completes, and the
  termination is what lets it complete. An earlier draft stated the two halves as
  an unordered conjunction;
- **the join is bounded** by `[delivery].fence-join-timeout-ms`, because
  replacement waits on the fence and an unbounded join would reintroduce the
  unbounded wait this change removes;
- **the escalation is fixed**: invoke the transport's **generation termination
  primitive**, whose contract is positive cessation of every effect path the
  generation owns. Reaping a child is ACP's and Pty's implementation of it, not
  the universal action — UI owns no child, and Tmux reaches its target through a
  server it must not kill. A timeout that only stops waiting establishes nothing;
  the primitive both unblocks an executor blocked writing into the terminated
  path and proves nothing in flight can still reach the target;
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
- `submission_unknown` MAY terminalize before the fence is positive — outcome
  terminality does not require it, so a negative fence strands no message;
- **replacement and normal ordering barriers SHALL NOT proceed** until the fence
  is positive.

That split is the *Decision 9* three-facts distinction applied to respawn:
resolving an outcome and proving execution ceased are different events, and only
the second may release a barrier or admit a new generation.

The guard is what makes "resolves exactly once" a property of the system rather
than an aspiration about well-behaved tasks.

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
| expired | `expired` | pre | relay patience elapsed; nothing authorized |
| dropped, transport unavailable | `transport_unavailable` | pre | relay policy; nothing authorized |
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
session is still registered — leaves members `Pending`, where residency governs
them and they resolve `expired` if the absence outlasts it. Otherwise
`transport_unavailable` would become another inference from absence, retired at
the transport and reintroduced at the relay.

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
terminal-outcome receipt exactly as `expired` and `dropped_on_shutdown` do.

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
| **Execution ceased** | fence/join/generation replacement | nothing on its own |
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

**Two raw modes**, because one rule cannot serve both purposes:

- **Normal raw** preserves FIFO and waits for target-side ordering safety.
- **Operator emergency raw** (Tmux and Pty only) **overtakes `Pending` mail and
  bypasses readiness gating.** It bypasses an in-flight submission only where the
  transport provides a separately supervised writer with a defined interleaving
  rule; no transport provides one today, so today it waits. ACP has no emergency
  raw — its recovery path is choose/cancel/teardown, not a second prompt.

That boundary is not a preference; it is what the transports permit. On Pty, raw
and envelope submission share one worker channel and one writer mutex
(`src/pty/transport.rs:168-180`; `src/pty/delivery.rs:357-368`, `:398-407`).
An emergency path cannot bypass a worker already blocked in submission: routing
around the mutex permits byte interleaving that corrupts both writes, and taking
the mutex blocks exactly as the normal path would. Tmux has the analogous
constraint on its command path. So emergency raw is scoped to what is achievable
without a second supervised writer — overtaking work that has not been authorized
— and a separately supervised independent writer with a defined interleaving rule
is named as follow-up work rather than assumed to exist.

**Operator surface:** an explicit mode on the `raww` contract, surfaced through
both the MCP tool and the CLI. It is a declared contract modification, not an
implicit behavior change to existing `raww` calls — an operator gets the ordering
break only by asking for it.

**Documented ordering break:** older `Pending` mail is neither retried nor
reclassified.

Emergency raw waits for target-side ordering safety of older in-flight execution
**wherever the transport provides no separately supervised writer** — which is
every transport today. An earlier draft said it does not bypass an in-flight
submission and also warned that an older authorized attempt might act after it;
both cannot hold, and the second was the wrong one to keep.

The condition is stated rather than the conclusion, because the conclusion is
phase-dependent and the condition is not. Overtaking an *unfenced* attempt is a
deliberate interleaving hazard in any phase; overtaking one under a supervised
writer's defined interleaving rule is the follow-on capability. Writing the rule
categorically would make the spec forbid what the follow-on implements.

**The operator-recovery capability is preserved in the core phase**, by
substitution rather than by carrying the old mechanism.

Pty raw today interrupts an in-flight envelope readiness wait
(`src/pty/delivery.rs:634-665`). That mechanism cannot be preserved literally,
because the wait it interrupts is exactly what moves to relay `Pending`; keeping
it would mean keeping an in-transport readiness wait and reopening the liveness
hole. After the move, a Pty submission is a millisecond-scale `write_all` pair
with nothing left to interrupt.

So the **core phase includes the raw mode discriminator and the minimum
relay-side ordering logic**: an explicit mode on the `raww` contract, plus
emergency mode overtaking that target's `Pending` mail and its readiness gate.
The capability at stake is *"my message is stuck waiting on a pane that will not
become ready, and I need to type past it"* — the stuck message simply moved from
an in-transport wait to relay `Pending`, and raw jumps it there instead.

**The discriminator cannot be deferred while the behavior ships.** Existing
`raww` carries no field distinguishing the two behaviors, so deferring it would
force one of three broken choices: make every raw overtake `Pending` (violating
normal FIFO and the explicit opt-in), keep FIFO for all raw (failing the Pty
recovery substitution the core promises), or add the discriminator anyway
(contradicting the phase boundary). Mode and behavior ship together or neither
does.

Core emergency mode overtakes `Pending` **only**, and waits for target-side
ordering safety of older in-flight execution using the fence that is also now in
core. Default and existing `raww` calls remain normal FIFO. This is surface
moved, not a new writer or a new race.

**Follow-on**: the independent supervised writer and the in-flight-overtake
extension that depends on it.

*Note on justification:* the core's liveness does **not** rest on a submission
being a "millisecond-scale write". It rests on submission primitives being
supervised, fenced, and interruptible per *Decision 1*. The short write is why
little is lost by not overtaking in-flight execution — not why the wait is
bounded.

Emergency raw exists because raw input is how an operator intervenes when
something is stuck, and a rule that makes intervention wait on the stuck thing is
the wrong rule.

**Acknowledged behavior change:** Pty raw today explicitly interrupts an
in-flight envelope wait (`src/pty/delivery.rs:634-665`). An earlier draft claimed
"no ordering change", which was false. Under normal raw that interruption is
removed, and core emergency mode replaces it — both ship in the core phase, so no
window exists in which the capability is absent.

### Decision 10 — The contract applies to every transport, and there are five

Earlier drafts said "all three transports" throughout. `TransportImpl` is `Tmux`,
`Acp`, `Pty`, `Ui`, and the forward-declared `Pubsub` stub. Writing "three" was
the same error as omitting five requirements from a sweep last round: enumerating
from the set already in mind rather than from the type.

**UI is fully in scope** for relay-owned queueing, readiness, capacity,
authorization, residency, and the ledger. It is not an edge case here — it is the
transport that most needs the change. Today it reports `Ready` unconditionally
(`src/transports/ui.rs:152-159`), spawns a thread per delivery, and resolves each
one through a **bounded reconnect wait** (`:180-188`). That wait is an
absence-adjudicating timer of exactly the kind this change retires: a subscriber
that has not reconnected within the window is not a subscriber that has failed.

Under this contract UI resolves like any other transport — `delivered` on a
broadcast accepted by at least one live subscriber, `not submitted` when there is
positively no live subscriber, and relay-side residency governing how long an
unreachable UI session's mail waits. The reconnect timeout is deleted, not
relocated.

`Pubsub` is a stub with no delivery behavior, and "inherits the contract when it
gains one" is not a specification. Its behavior is stated concretely: a `Pubsub`
target is **rejected synchronously at admission** with the existing
not-implemented error, before anything is queued or authorized. It produces no
terminal outcome and no receipt, because nothing was ever accepted — which is why
it needs no new outcome spelling. **Work SHALL NOT be authorized merely to
discover the stub.** It is inside the uniform contract with a defined answer,
rather than silently outside it.

## Risks / Trade-offs

- **`submission_unknown` is pessimistic**, covering both "never started" and
  "wrote and died". → Accepted; the alternative is an acceptance protocol whose
  race is the complexity this design avoids. Revisit when a wire transport needs
  an acknowledgment.

- **This retires `bound-tmux-readiness-wait`, merged days ago.** → The bound fixed
  a real unbounded wait; what it got wrong was the *location* of the judgement.
  The residency policy replaces it with the same guarantee stated honestly.

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

- **Five config keys deleted across the three coder transports.** → Coordinator
  prepares both `coders.toml` files against the settled list before any restart.
  UI adds nothing to that list: its reconnect timeout is a constant plus builder
  (`src/transports/ui.rs:129-147`), not a TOML key, so retiring it is a code
  deletion rather than a config break.

- **The contract now reaches five transports, and UI's per-delivery threading and
  reconnect wait are a larger change than the coder transports'.** → In scope per
  *Decision 10*; carving UI out would leave an absence-adjudicating timer alive in
  the one transport whose "absence" is just a browser tab that closed.

## Open Questions

None blocking. Capacity units, residency policy placement, and crash-recovery
scope were previously listed here and are now settled in *Decisions 5 and 8*;
carrying them as open questions was itself a review finding.

Deliberately excluded, each a follow-up rather than a gap: per-target residency
overrides, durable crash recovery, and transport-side attempt-ID deduplication
that would allow a stronger guarantee than *Decision 6*.
