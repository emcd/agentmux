# Relay Delivery — Architectural Decision Record

Why the delivery contract has the shape described in
[`delivery-architecture.md`](delivery-architecture.md). Superseded decisions
stay, marked as superseded, because the reasoning that was rejected is usually
the reasoning someone proposes again.

Each entry records what was given up, not only what was chosen.

**This file is closed to new entries.** Closed decisions go to
`documentation/decisions/`, per `documentation/development-practices.md`.
Migration of D1-D11 to that home is pending and is not being done entry by
entry here; until it happens this file remains the authority for the decisions
it holds. An entry written to the new home that continues a thread started here
is named below, so a reader following the thread reaches it.

- D12 → `documentation/decisions/0004-the-mailbox-holds-the-delivered-artifact.md`

---

## D1 — Retire every absence-inference timer

**Status:** decided, implemented

Three separately named defects were one defect: **a transport inferring delivery
failure from the absence of change, and reporting it as non-delivery for bytes it
may already have committed.**

- *Wedge detection* inferred failure from an unchanged screen. On Pty it reached
  that verdict in under a second, so an agent sitting on a tool-permission dialog
  failed its deliveries almost immediately.
- *Prime timeout* inferred failure from absence of output. A pane awaiting a
  keystroke is silent and perfectly healthy.
- *Readiness timeout* inferred "gave up" from a prompt that never returned —
  honest on Tmux where nothing was injected, but still abandoning a healthy
  long-horizon agent turn while phrasing it as a fact about the pane.

The live `Delivery Classifier` requirement already stated the rule these violated:
only a positively observed terminal event — process death, a closed connection, a
protocol error — is sound evidence of failure, and an unchanged screen is not. It
sat four paragraphs above a timer that fired on absence.

**Given up:** the timers were also the only thing bounding a stuck delivery.
Retiring them without a replacement is what generated most of the machinery in
D3–D5. That cost was paid link by link and was not priced in aggregate at the
time — see D10.

---

## D2 — Put no bound in their place

**Status:** decided, implemented

An earlier draft replaced the timers with a relay-level residency bound resolving
an `expired` outcome. **Rejected**: that is the same inference relocated. Elapsed
waiting still decided an outcome, and because expiry terminalizes and releases
quota, it dropped mail that would have landed once a long agent turn finished.

A `Pending` entry whose target is reachable now waits indefinitely. The `expired`
outcome was deleted along with the timers.

**What replaced them** is not a bound at all: relay-level admission quota
enforced positively at send time, plus undelivered-queue inscriptions that report
a long wait without adjudicating it.

**Given up, explicitly:** a `Pending` entry for a reachable-but-never-ready target
holds its admission quota forever. Per-target quota is the bound on the
*consequence*, not on the wait. A target busy long enough will start refusing new
sends at admission. This is documented operator-facing behaviour, not an
oversight.

**No configuration key may be added that bounds how long a delivery waits for a
reachable target.** This is a standing constraint on future work, not a current
implementation state.

---

## D3 — `Authorized` exists because the relay must give up the right to reclaim

**Status:** decided, implemented, superseded by D11 — the line this state drew
survives; the state itself does not.

`Authorized` is frequently misread as a handshake with the transport. It is not:
the transport is never told about it and never acknowledges it. It is the relay
making a promise **to itself** — *from here on I will not take this message back.*

That is the entire content of the state, and it maps directly onto the defect the
contract exists to fix:

- **Before** authorization the relay may truthfully report non-delivery. Nothing
  was written, so the statement is true.
- **After** authorization it may not. Bytes may already be moving, so any
  relay-side decision to call it undelivered risks being a lie.

`Authorized` is the line between "safe to report non-delivery from policy" and
"must report from evidence." `agentmux:issues/relay/62` is the same error from
the other side — Pty reporting `Delivered` for bytes that never left the relay.
Both are reporting from position rather than from evidence.

**Given up:** an irreversible state requires an owner. See D4.

---

## D4 — The guard cannot be deferred past `Authorized`

**Status:** decided, implemented, superseded by D11 — the governing invariant
holds unchanged; only the transition it is anchored to has moved.

An earlier draft deferred the authorization guard on the grounds that
exactly-once resolution is already not guaranteed today. That was wrong on a
dimension it did not check.

This change introduces per-target and relay-global quota in count and bytes which
**releases only at the terminal transition**. An unowned `Authorized` entry
therefore does not merely lose a receipt — it **leaks quota permanently and
blocks the target FIFO**, since it can neither expire nor retry. Today's
collector panic at least releases its pending slot. Deferring the guard would
*regress* resource accounting rather than hold it constant.

**The governing invariant:** *no transition to `Authorized` may occur unless an
owner capable of terminalizing and releasing it is created in the same atomic
operation.*

**Corollary, recorded so it is not rediscovered:** if the minimum guard cannot fit
a phase, the correct response is to defer the irreversible `Authorized` model
**and** the timer retirement together — not to ship an unowned state.

---

## D5 — Sustained unreachability is bounded; unreadiness is not

**Status:** decided, implemented

`[delivery].unreachable-dwell-ms` resolves members whose target has been
continuously `Unreachable` past the threshold. This looks like a timer and is
routinely challenged as one.

The distinction is epistemic, not mechanical. Duration here **qualifies a
repeatedly-made observation** rather than **substituting for an absent one**:

- Unreadiness: we observe nothing. A silent pane and a working agent are
  indistinguishable. Elapsed time adds no information, so a bound would be pure
  inference — exactly D1's defect.
- Unreachability: we observe a failure, repeatedly, every probe. The dwell asks
  how long that positive observation has persisted.

*Sustained unreachability is itself evidence, in a way that sustained busyness is
not. Same clock, opposite epistemics.*

Any return to `Healthy` resets the dwell, so a transient unreachability resolves
nothing.

---

## D6 — Withdraw cross-target round-robin scheduling

**Status:** decided, withdrawn from scope

The proposal specified a byte-budgeted round-robin across targets, with a
`scheduling-quantum-bytes` key.

It was withdrawn after the question *"if transports signal readiness, what is the
scheduler for?"* went unanswered. Each target is served by its own worker; the
relay arbitrates between none of them. A scheduler polling readiness flags would
duplicate the notification closure the transports already invoke.

The quantum had a defect of its own: it had to be at least as large as the
largest handover byte component, while batch formation was already capped there —
so it could never bind on a first batch. Two limits on one quantity, the same
defect the design had already caught once in a deficit counter.

`scheduling-quantum-bytes`, its default, range, and load-time validation were all
deleted.

**Given up:** nothing observable. No fairness property was lost because no
contention point existed to be fair about.

**Correction recorded:** an early defence of this withdrawal claimed targets "do
not contend." That was false. The counterexamples raised at the time were Pty's
blocking submission enqueue, the shared bundle tmux socket, and ACP bootstrap's
shared blocking pool. The Pty instance has since been fixed — `mailw`/`raww` now
use `try_send`, and the remaining Pty `blocking_send` calls are snapshot
requests, child-output forwarding, and a test channel fill, none of them on the
relay submission path. The shared tmux socket and the ACP bootstrap pool remain
live contention points.

The conclusion survived on different ground regardless: no elapsed duration
converts unreadiness into an outcome, whether or not targets contend.

---

## D7 — Execution watchdog over per-transport stalled-execution health

**Status:** decided 2026-08-10, watchdog retained

The one gap the two-condition model (relay shutting down / transport unhealthy)
does not close: an executor **alive, with a healthy target, stuck in our own
code**. Concretely, a worker parked in a blocking write to a target's pipe when
the child has stopped draining and the buffer is full — Pty's `write_all` to the
master, ACP's `write_line_to_stdin` to the child's stdin under a shared mutex.
Both sites carry comments acknowledging the state. Neither shutdown nor
unhealthy fires; the member stays `Authorized` forever, holding quota and
blocking the target FIFO.

Two candidate answers were argued at length.

**Candidate A — non-blocking writes plus a health dwell.** Convert the blocking
writes, treat sustained non-writability as a `TransportHealth` transition, let
the existing fence tear down the stream, and delete `submission-timeout-ms`.

**Rejected**, on two grounds, both from AuxBE:

1. Non-blocking descriptors stop an OS thread parking; they do not make a
   submission *complete*. A partial write is already a target-side effect but not
   a valid message, and it has no sound classification — `Submitted` is untrue,
   `not_submitted` is false, and `submission_unknown` plus reuse is unsafe
   because the retained suffix can later complete or interleave. With a 256 KiB
   handover maximum against a typical 64 KiB pipe buffer, mid-frame stalls are
   reachable rather than theoretical.
2. It relocates the liveness problem into a pending writer future rather than
   ending it. A timer declaring that future unhealthy would be an execution
   watchdog by another name.

**Candidate B — the specced authorization-anchored watchdog.** Retained, decided
on implementation cost rather than elegance. Once submission evidence is recorded
at the successful write boundary, the watchdog does not fire on a healthy agent's
subsequent long turn, and it reuses the fence that is already built. Candidate A
would need a new per-transport stalled-execution model, partial-write ownership
and cessation integration, a new dwell policy, coverage of the Tmux `read_to_end`
case, and a proof that tmux client death cuts server-owned deferred effects that
neither reviewer could establish from source.

**Given up:** the watchdog is a duration-anchored trigger, which sits uneasily
beside D1 and D2. It is admissible only because it bounds **the relay's own
supervised code** between authorization and evidence, states nothing about target
health, and produces no failure spelling. It must not arm before submission
evidence lands — until a transport records `Submitted` at write time, its outcome
future resolves only after the target has finished responding, so a bound
anchored at authorization would measure the agent's inference and fence a healthy
target mid-turn.

**Conditions carried onto the watchdog work** (all from candidate A's analysis,
all applying regardless of which answer won):

- The authorization-to-write TOCTOU window is real on ACP and Pty; both gate
  reads are explicitly advisory.
- Tmux `drain_invocation_pipes` parking in `read_to_end` is a distinct unbounded
  point from the reap loop, and any completion predicate must cover both.
- Whether tmux client death cuts server-owned deferred effects remains
  unestablished. The Tmux fence verdict inherits that gap.

**Arming, 2026-08-10.** The precondition was checked against source rather than
against the task list before arming: all three transports resolve their outcome
future at the write boundary (`acp/transport.rs` `submit_envelope_turn`,
`tmux/transport.rs` at `inject_literal_text`, `pty/delivery.rs` `write_unit`), and
ACP's respawn gap needs no exemption because `mailw` refuses synchronously with no
live runtime rather than holding an authorized member across one.

Two consequences of arming that were not visible from the decision itself, both
following from what `generation_ceased` requires per transport:

- **A fenced Pty generation costs the coder child.** Pty's cessation predicate
  includes `child_reaped`, so a positive verdict means the child is already dead
  and the replacement generation spawns a fresh one. The coder loses its session
  state. This is not gratuitous — reaching the watchdog at all means an executor
  was parked in a write the child had stopped draining — but it is a real cost
  that a Tmux fence does not carry, since Tmux's predicate is over its own threads
  and the pane survives untouched.
- **A negative verdict is permanent for the process.** The target's registry entry
  is retained deliberately, because registration is the election a spawner must
  win; holding the key is the whole no-replacement mechanism. Recovery is a relay
  restart. Chosen over the alternative in full knowledge: a target that accepts
  nothing is recoverable by operator action, and a target that two generations may
  be writing to concurrently is not.

**Re-anchored by the pull-model cutover.** The bound above was described as
running from "the point a packing unit's write begins", and the arming note
checked its precondition against the transports' outcome futures. Neither survives
literally: the relay no longer invokes a transport, so it cannot observe a write
beginning, and there is no outcome future to resolve at the write boundary. The
bound now runs from the moment the relay **accepted a declaration** — the last
instant before any target-side effect, and the one end of the interval the relay
can still see. What the precondition becomes is that each transport's `write`
returns at its write boundary rather than at the end of a turn, which is why ACP
splits its turn observation out into the next iteration's readiness check. ACP's
respawn gap needs no exemption for the same reason as before, differently
realised: with no live runtime its executor's readiness check withholds every
write, so entries stay queued and undeclared and nothing is bound across the gap.

---

## D8 — ACP is excluded from emergency raw, on protocol grounds

**Status:** decided, corrected 2026-08-10

Emergency raw mode overtakes `Pending` mail and bypasses readiness gating on Tmux
and Pty. ACP, UI, and Pubsub reject it.

The exclusion was briefly challenged as unprincipled, on the theory that it was
merely an artifact of raw and mail sharing one ordered channel — which is equally
true of Tmux and Pty. **That challenge was wrong.** The real reason is protocol
shape:

- ACP raw is implemented as another `session/prompt`, and the client enforces one
  active prompt. Bypassing readiness while an older turn is active yields a
  serialization refusal, not steering.
- ACP has no byte-stream or key-input primitive analogous to Tmux and Pty. There
  is nothing to "type past" with.
- An ACP agent blocked on a tool-call approval — the case that most looks like it
  needs steering — resolves through the relay-injected `Chooser`, not through
  text. Arbitrary raw text cannot safely enter that protocol turn. `choose` is the
  correct surface.

The useful distinction is **not** per-transport but per-sense:

- Overtaking **`Pending` mail** is relay-side queue reordering. It is cheap,
  needs no new writer, and is mechanically transport-agnostic — but it delivers
  *steering* only where the transport can accept the resulting raw handover. ACP
  is the counterexample described immediately above: reordering the queue merely
  moves the raw `session/prompt` to the front, where an active turn fails it on
  single-flight serialization. Mechanically possible everywhere; useful on Tmux
  and Pty.
- Overtaking an **in-flight submission** needs a separately supervised writer with
  a defined interleaving rule. No transport provides one; deferred to 0.9.x.

---

## D9 — Per-target FIFO is worker-enqueue linearization

**Status:** decided, implemented, tested

FIFO per target is guaranteed, defined as **worker-enqueue linearization** — not
request order and not admission order.

`try_existing_worker` holds the registry lock across `sender.send`, and the
channel is unbounded so the send cannot block; therefore channel order equals
lock-acquisition order. Mail and raw are one order because both reach it through
`enqueue_async_delivery`.

**Given up:** a request may reserve admission quota first and still lose the
enqueue race. Admission order is deliberately *not* the guarantee, because
admission and enqueue are separate operations and making them one would
serialize admission behind the registry lock.

---

## D10 — On how this change grew

**Status:** recorded, not a technical decision

Recorded at the operator's request, because the growth pattern is more reusable
than any single decision above.

The change began as "remove some erroneously-triggered timeouts" and became a
delivery commit protocol. The chain:

> delete the timers → the wait must live somewhere observable → relay-owned queue
> → unbounded queues need capacity accounting → admission quota → quota releases
> only at the terminal transition → an entry that never terminalizes leaks it →
> guard → the guard needs to know what happened → typed submission evidence → a
> blocked-but-alive executor produces no event → watchdog → the watchdog cannot
> just kill things → fence.

Every arrow is load-bearing. None was priced against the one before it. The total
was never re-examined until the operator asked whether the scheduler should exist
at all (D6), which removed nine tasks in one question.

Two halves got welded together and are worth separating when reasoning about
scope:

- **The defect fix** — evidence recorded at write time, per packing unit, each
  member resolved from its own record. This is `issues/relay/61` and `/62`, the
  original complaint, and a minority of the work.
- **The machinery that makes indefinite waiting safe** — queue, quota, guard,
  watchdog, fence, generation replacement, raw mode. All downstream of D2 alone.

The second half is not what was asked for. It is what "no bound at all" costs.
D2 remains the right call; the cost should be named rather than absorbed
silently.

---

## D11 — `Authorized` collapses into declaration; the line it drew survives

**Status:** decided, implemented

D3 justified `Authorized` as the relay's promise to itself — the line between
"safe to report non-delivery from policy" and "must report from evidence." The
pull model keeps that line and deletes the state.

Under the push model the relay chose when to hand a member over, so it needed a
state recording that it had. Under the pull model a transport's executor decides
what to write and says so first, by declaring the exact contiguous range it is
about to submit. Declaration lands at the same point in the sequence and carries
the same meaning: before it the relay can prove nothing was written; after it,
partial effect cannot be excluded. What changed is who initiates, not where the
line falls.

So an entry now has two states, `Queued` and `Terminal`, and a declared entry is
still `Queued`. Declaration is metadata the entry carries — which packing unit it
is bound to — rather than a lifecycle position. Modelling it as a third state
would recreate the problem D4 names from the other direction: a state is
something an entry must be able to *leave*, and the only event that resolves an
entry is acknowledgment. A third state would be one no event owns.

D4's governing invariant survives unchanged, reworded to its new anchor: *no
declaration may bind a packing unit unless an owner capable of terminalizing and
releasing it is created in the same atomic operation.* Declaration is still the
guard's creation point; it moved earlier and changed initiator, not owner.

**Given up:** the composite `(batch, attempt)` member identity. A member is now
named by its own mailbox position. The attempt component existed to support the
claim of *at most one relay-authorized injection attempt*, which needed to
distinguish one authorization of a member from another. Acknowledgment is
idempotent per entry, so a second acknowledgment of an entry already terminal is
a no-op rather than a second attempt — an identifier separating them would only
ever be read to conclude that it did not matter.

**Cost, named rather than absorbed:** the relay's own bookkeeping no longer
records a delivery *attempt* distinctly from the entry it was for. Nothing today
reads that distinction, but a future transport that reports its own partition
back would be describing attempts the relay cannot name. That is a reason to
revisit the identity then, with the requirement in hand, rather than to keep an
unused component now.

---
