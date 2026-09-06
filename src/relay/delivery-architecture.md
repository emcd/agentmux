# Relay Delivery Architecture

How a message travels from a `send` call to a terminal outcome, and where the
relay's responsibilities end and a transport's begin.

Companion document: [`delivery-decisions.md`](delivery-decisions.md) records
*why* the shape is this way. This file records *what* the shape is.

**The relay never invokes a transport to deliver.** It holds an ordered mailbox
per target and fills it; each transport owns one serial delivery-loop executor
that peeks that mailbox, decides what to write, declares the run it decided on,
writes it, and acknowledges what the write proved. There is exactly one delivery
path, and the diagrams below draw it.

Each admitted entry's payload is built once, at the delivery worker's task
intake, and placed in its target's mailbox — so what an executor writes is the
artifact the mailbox holds rather than one it rendered a second time.

Diagrams are ASCII rather than Mermaid so they read identically in a terminal,
an editor, and a web view. The repository uses no Mermaid elsewhere.

## The four events

The contract's central move is separating four things that delivery code
historically collapsed into one. Each has a different owner and a different
truth condition.

```
   ADMISSION            DECLARATION             SUBMISSION            RESOLUTION

   accept into queue    record the run about    a packing unit        each member
   reserve quota        to be written, before   produces a            resolves once,
   fix mailbox position any target-side effect  target-side effect    from evidence
   return "queued"      the linearization
                        point

   relay owns           relay records,          transport owns        relay owns
   synchronous          transport decides       observed, reported    guard + CAS
                        never reclaims after
```

The rule that makes this worth the machinery: **before a declaration the relay
may truthfully report non-delivery, because nothing was written. After one it may
not, because bytes may already be moving.** Reporting from position rather than
from evidence is the defect this whole contract exists to remove
(`agentmux:issues/relay/61`, `agentmux:issues/relay/62`).

**An entry has two states, not three.** `Queued` and `Terminal`, and the
transition between them is one-way. A declared-but-unacknowledged entry is still
`Queued`: which packing unit it is bound to is metadata the entry carries, not a
lifecycle position. A third state would have to be *left* by something, and the
only thing that may resolve an entry is acknowledgment — so a state between the
two would be one no event owns. What discriminates a waiting entry from one being
written is therefore the guard, not the state.

## Component layout

```
  caller (CLI / MCP / TUI / peer relay)
     |
     |  send
     v
  +--------------------------------------------------------------+
  |  ADMISSION                          relay/delivery/admission.rs |
  |                                                              |
  |  - reject Pubsub synchronously                               |
  |  - reject oversized canonical payload                        |
  |  - reserve per-target + global quota                         |
  |    (count and bytes, atomically)                             |
  |  - fix the entry's position in its target's mailbox,         |
  |    because admission is what linearizes two concurrent       |
  |    sends against each other                                  |
  |                                                              |
  |  returns "queued" -- NOT "delivered"                         |
  +--------------------------------------------------------------+
     |
     |  enqueue (unbounded mpsc, one channel per target)
     v
  +--------------------------------------------------------------+
  |  PER-TARGET MAILBOX                                          |
  |                                                              |
  |  Entry states: Queued | Terminal                             |
  |  relay/delivery/guard.rs :: QueueEntryState                  |
  |                                                              |
  |  A Queued entry holds its quota reservation, whether or not  |
  |  a packing unit has been declared over it. It leaves         |
  |  Queued ONLY by:                                             |
  |    - acknowledgment                                          |
  |    - positively observed transport teardown                  |
  |    - sustained unreachability past the dwell                 |
  |    - graceful shutdown                                       |
  |  No elapsed duration resolves a Queued entry whose target    |
  |  is reachable. This is deliberate; see delivery-decisions.md |
  +--------------------------------------------------------------+
     |
     v
  +--------------------------------------------------------------+
  |  DISPATCH WORKER            relay/delivery/dispatch/worker.rs |
  |  one tokio task per target -- PRODUCE ONLY                   |
  |                                                              |
  |  It receives a task, builds its payload once, places it in   |
  |  the target's mailbox, and rings the doorbell. It reads no   |
  |  readiness, invokes no transport, and collects no outcome.   |
  |                                                              |
  |  What stays here is custody and supervision:                 |
  |    - the mailbox and the consumer generation                 |
  |    - the submission-timeout watchdog, anchored at the        |
  |      moment a declaration was accepted                       |
  |    - the fence, fail-stop, and generation replacement        |
  +--------------------------------------------------------------+
     |
     |  the doorbell rings; correctness never depends on it
     v
  +--------------------------------------------------------------+
  |  DELIVERY-LOOP EXECUTOR         one per transport instance,  |
  |  transports/contract/executor.rs      serial, on its own     |
  |                                       thread, for its life   |
  |                                                              |
  |    peek -> decide/render/measure -> declare -> write -> ack  |
  |                                                              |
  |  health()      -> Healthy | Unreachable{since}               |
  |     past dwell   -> resolve_unreachable(): queued entries    |
  |                     resolve not_submitted                    |
  |     within dwell -> hold; the entries stay queued            |
  |                                                              |
  |  is_ready()    -> false: hold, declaring nothing             |
  |                                                              |
  |  Health says WHETHER the target can be reached at all.       |
  |  Readiness says WHETHER it will take a write now.            |
  |  Only the first is a reason to stop waiting.                 |
  |                                                              |
  |  Neither reading is advisory any more: the executor that     |
  |  takes them is the one that writes, so nothing can go stale  |
  |  between the check and the write.                            |
  |                                                              |
  |  Owns: coalescing, packing, rendering, the actual write.     |
  |  The relay compiles no prompt regex, inspects no pane        |
  |  output, and compares no cursor column.                      |
  |                                                              |
  |    Tmux  load-buffer + paste against the resolved pane       |
  |    Pty   write_all to the pty master                         |
  |    Acp   framed session/prompt to the child's stdin          |
  |    Ui    broadcast to the generation's subscribers           |
  +--------------------------------------------------------------+
     |
     |  declare()  <-- THE LINEARIZATION POINT
     |  recorded before any target-side effect; binds the guard
     |
     |  ack() reports one evidence per member
     v
  +--------------------------------------------------------------+
  |  RESOLUTION                                                  |
  |                                                              |
  |  Single terminal CAS. Quota releases HERE and nowhere else.  |
  |  The cursor advances HERE and nowhere else.                  |
  |                                                              |
  |  An acknowledgment reports what the write observed for each  |
  |  member, and each member resolves from its own report. The   |
  |  guard's resolution order runs only for a lifecycle trigger  |
  |  that brings no outcome of its own:                          |
  |    not_submitted        (member never declared into a unit)  |
  |    else submission_unknown                                   |
  +--------------------------------------------------------------+
```

## Delivery sequence

The normal path, and the two ways it diverges.

```
 caller        relay          dispatch worker      executor        target
   |             |                  |                  |             |
   |-- send ---->|                  |                  |             |
   |             |                  |                  |             |
   |        [admission]             |                  |             |
   |        reserve quota           |                  |             |
   |        fix mailbox position    |                  |             |
   |<-- queued --|                  |                  |             |
   |             |                  |                  |             |
   |             |--- enqueue ----->|                  |             |
   |             |                  |                  |             |
   |             |             [intake] build payload  |             |
   |             |             place in mailbox        |             |
   |             |             ring the doorbell ----->|             |
   |             |                  |                  |             |
   |             |                  |            health() -> Healthy |
   |             |                  |            is_ready() -> true  |
   |             |                  |                  |             |
   |             |                  |<---- peek -------|             |
   |             |                  |----- run ------->|             |
   |             |                  |                  |             |
   |             |                  |            [plan] decide how   |
   |             |                  |            much of the run one |
   |             |                  |            write may carry     |
   |             |                  |                  |             |
   |             |                  |<--- declare -----|             |
   |             |            [LINEARIZATION POINT]    |             |
   |             |            guard created            |             |
   |             |            quota now held by guard  |             |
   |             |                  |----- unit ------>|             |
   |             |                  |                  |             |
   |             |                  |                  |-- write --->|
   |             |                  |                  |             |
   |             |                  |<---- ack --------|             |
   |             |                  |   one evidence per member      |
   |             |                  |                  |             |
   |             |            [terminal CAS]           |             |
   |             |            quota released           |             |
   |             |            cursor advances          |             |
   |             |<-- receipt ------|                  |             |
   |             |                  |                  |             |
```

### Divergence A — target not ready

```
   |             |                  |            is_ready() -> false |
   |             |                  |                  |             |
   |             |            nothing is peeked, nothing declared    |
   |             |            the entry stays queued and undeclared  |
   |             |            quota stays reserved                   |
   |             |            NO timer runs                          |
   |             |                  |                  |             |
   |             |            (the executor re-reads its own level   |
   |             |             on the next iteration; the doorbell    |
   |             |             and a bounded poll both wake it)       |
```

**The poll is the guarantee, not the notification.** The executor waits on its
doorbell or on `poll_interval`, whichever comes first, and peeks either way on
the next iteration — so a lost ring costs the poll interval and nothing else.
Correctness never depends on a ring arriving. `TransportImpl::tmux` additionally
takes an `Option<ReadinessNotifier>` so a change in observed pane readiness can
shorten that latency; treat any such wakeup as an optimisation, on every
transport.

**Readiness is re-read between units within one drain.** The coder transports
publish `Busy` on accepting a write, so a drain that asked once would decide the
second unit's fate on an observation the target had already invalidated. That is
the defect this whole redesign exists to close, and it is closed from inside the
executor rather than by a rule the relay applies.

**Register-before-fill is structural, not merely intended.** The worker registers
its target's doorbell in `build_generation`, before the transport that will wait
on it is constructed and therefore before anything can peek — so a ring cannot be
issued against a generation that has no handle to receive it. The bounded poll
remains the lost-wakeup backstop either way.

The sender is told `queued` and hears nothing further until the target becomes
ready. This is the behaviour that replaced the prime/readiness/wedge timers. A
long agent turn produces a backlog of queued entries in the target's mailbox —
**not** a backlog in the target's pipe, because the executor's readiness check
never let the write start.

### Divergence B — target continuously unreachable

```
   |             |                  |    health() -> Unreachable{since} |
   |             |                  |                  |             |
   |             |            since.elapsed() >= unreachable_dwell?   |
   |             |                  |                  |             |
   |             |            no  -> hold; entries stay queued        |
   |             |                  |                  |             |
   |             |            yes -> the executor calls               |
   |             |                  |<-- resolve_unreachable() -------|
   |             |            queued-and-undeclared entries resolve   |
   |             |            not_submitted, reason_code:             |
   |             |            delivery_target_unreachable             |
   |             |            quota released                          |
```

The executor reports only that the condition it alone can observe has held long
enough; the relay owns the transition and chooses the outcome. A declared entry
is left to the guard's evidence order rather than resolved here, because a write
may already have reached the target for it. The call is idempotent, which is what
lets it be driven by a continuously-observed condition rather than by an edge the
executor would have to detect.

Any return to `Healthy` resets the dwell, so a transient unreachability resolves
nothing.

## Reporting the partition

The relay hands over one envelope at a time and cannot see what a transport does
with them. ACP coalesces a budget group into one `session/prompt`; Tmux splits a
batch into token-budgeted pastes; Pty writes each member separately. That
partition decides which members share a fate, so the guard has to learn it from
the layer that chose it.

`MailboxConsumer` (`transports/contract/executor.rs`) is that seam, injected as an
`Arc<dyn MailboxConsumer>` at transport construction — the same shape as ACP's
`MirrorStateFn` and Pty's `PtyMirrorStateFn`, and for the same reason:
`src/transports` may not import `crate::relay`. The relay's implementation is
`relay/delivery/consumer.rs`, which delegates to the admission ledger.

The handle closes over the target and the consumer generation rather than taking
them per call. The check that a call belongs to the target's active generation is
unchanged — the relay still compares the binding under its own lock — but a
transport that cannot name a target cannot name the wrong one, and a generation
identifier that never reaches the transport cannot be retained past its
replacement.

Two calls bracket the write:

```
  declare(range) -> Result<DeclareAccepted, DeclareRejection>
      before the first target-side effect
      binds the whole contiguous range at the cursor, or none
      Err obliges the executor to produce NO effect for that unit

  ack(unit, &[MemberAcknowledgment]) -> AckResult
      unconditional once a unit is declared
      one evidence per member, in the order the plan covered them
      the acknowledgment must cover the declared range exactly
```

`declare` refuses rather than binding a subset, and that asymmetry is the point.
Binding *late* costs precision — the evidence order falls back to
`submission_unknown` once a member is bound. Binding *partially* costs
correctness: a member that terminalized a moment earlier would resolve
`not_submitted`, a positive claim that nothing was written, while the executor
went on to write the run it had already composed.

`ack` is unconditional, and that is the whole reason `write` is split from
`plan`: a declared unit whose write failed observably is acknowledged with what
that failure proved rather than left to the execution watchdog. The watchdog
remains the backstop for an executor that cannot report at all — one that has
panicked, and is therefore not there to call `ack`.

What the seam enforces is that an acknowledgment covers its unit exactly: every
declared position once, and nothing else. A missing member has no report to be
resolved from, and the only ways to proceed would be to invent one or to borrow a
sibling's, both of which state an outcome nothing observed for that member. What
it cannot enforce is the ordering before `declare` — nothing stops a transport
side-effecting first and declaring afterwards. That stays a per-transport
boundary test.

Every transport reports its own partition now; there is no relay-declared
fallback, because there is no relay-side write left to declare for.

| Transport | Unit |
|-----------|------|
| Tmux | one token-budget group per paste |
| ACP | one budget group per `session/prompt` |
| Pty | one entry per write |
| UI | one entry per stream event |

Pty's one-entry-per-unit is a commitment rather than a limitation: coalescing
would smear a partial write's evidence across members, since an earlier member's
bytes could land while a later member's did not and a shared unit could only
report one answer for both.

### Members no reservation covers

A terminal-outcome receipt bypasses admission, so it holds no quota reservation —
but it does take a mailbox position, and it is declared and acknowledged like any
other entry.

That is a change of shape from the push model and it is forced. Under a pull
model an executor writes only what it peeks, so an entry with no position is an
entry nothing delivers: a positionless receipt would be silently undeliverable,
and the drop would be invisible, since receipts are relay-originated and nothing
downstream waits on one. What such an entry lacks is a reservation, and the only
thing that follows from lacking one is that its acknowledgment releases none.

## Generation fencing

A transport *generation* is one instance of a transport plus everything it owns —
child processes, executors, threads. Replacing a generation (respawn) or shutting
down requires proving the old generation stopped, because an un-stopped executor
can still produce a target-side effect attributed to the wrong generation.

Five steps, in `relay/delivery/fence.rs` and the per-transport
`terminate_generation` / `generation_ceased` pair.

```
  1. cooperative stop request        set the fenced flag
  2. bounded cessation observation   fence-observation-timeout-ms
  3. forced termination              non-blocking, INITIATION ONLY
                                     Acp/Pty: signal the child
                                     Tmux:    signal owned client invocations,
                                              never the server
                                     Ui:      drop broadcaster + subscribers
  4. second bounded observation      same clock, own window
  5. verdict                         positive ONLY on observed cessation
```

Two rules that are easy to get wrong and are load-bearing:

- Steps 1 and 3 stay distinct, so a cooperatively stoppable executor is never
  force-terminated.
- A successful step-3 invocation does **not** acknowledge the fence. Only
  observed cessation in step 4 does. Both timeout and failure route to the
  negative branch.

Two things initiate a fence: graceful shutdown, and the execution watchdog
described below.

A **positive** verdict tears the old generation down and builds a replacement in
place, from the same source the worker's first generation was built from — so a
target cannot acquire a second transport kind by being fenced. A **negative**
verdict marks the target fail-stopped:
its registry entry is held for the rest of the process's life, which is what makes
a replacement unelectable, and every further send is refused with
`delivery_target_fail_stopped`. Raw needs no separate barrier — a raw entry
reaches the target through that same registry lookup. Either verdict resolves
every still-unresolved member through the guard.

## The execution watchdog

The one gap the two-condition delivery model (relay shutting down / transport
unhealthy) cannot close: an executor **alive, with a healthy target, stuck in our
own code** — parked in a blocking write whose buffer is full. Neither condition
fires and the unit stays declared forever, holding quota and blocking the target
FIFO.

The bound is anchored at the moment the relay **accepted the declaration**, which
is a relay-observed event. The push model anchored it at "the point a packing
unit's write begins", which the relay cannot observe at all now that the write
happens inside a transport's own executor; a declaration's acceptance is the last
instant before any target-side effect, so it bounds the same interval from the
one end the relay can see.

```
   declaration ──────────────────────────────────► acknowledgment
        │                                                ▲
        │◄────── submission-timeout-ms ──────►│          │
        │                                     │          │
        │                              elapsed, still    │
        │                              unresolved        │
        │                                     │          │
        │                                     ▼          │
        │                          initiate generation fence
        │                          terminalize NOTHING here
        │                                     │          │
        │                          ┌──────────┴───────┐  │
        │                          │ evidence still   ├──┘
        │                          │ admissible       │
        │                          │ through both     │
        │                          │ observation      │
        │                          │ windows          │
        │                          └──────────┬───────┘
        │                                     ▼
        │                              fence verdict
        │                        THE SINGLE RESOLUTION CUT
        │                    still-unresolved members terminalize
        │                    through the guard's evidence order
```

Anchored at authorization, so the bound covers the whole supervised path including
rendering and submission. It is admissible beside the "no elapsed duration decides
anything" rule only because it bounds **the relay's own code**: it states nothing
about target health and produces no failure spelling. Its members resolve
`submission_unknown` because not knowing is what actually happened.

It measures relay execution rather than the agent's inference only because every
transport's `write` returns at the write boundary — ACP after the framed
`session/prompt` write, with the turn's completion waited out separately in the
next iteration's readiness check; Tmux at the `inject_literal_text` invocation;
Pty at the buffered `write_all`. Break that property in any transport and the
watchdog starts fencing healthy targets mid-turn.

Terminalizing at the bound rather than at the verdict would destroy evidence the
fence is about to produce: a bound member with no record wins `submission_unknown`,
and a cooperative stop that then proves nothing was written could no longer be
accepted. One cut at the verdict preserves the evidence order instead of racing
it.

## What the relay does not do

Recorded because each was true at some point and is now deliberately false:

- The relay runs no cross-target scheduling. Each target has its own worker and
  the relay arbitrates between none of them. A byte-budgeted round-robin was
  specified and withdrawn; see `delivery-decisions.md`.
- The relay compiles no prompt regex and inspects no pane content. Prompt
  readiness lives entirely inside each owning transport.
- The relay holds no carry buffer and does no coalescing. Both live in the
  transport's delivery-loop executor.
- The relay reads no readiness level from any transport, and decides for none of
  them when to write. It fills a mailbox; the executor decides.
- No configuration key bounds how long a delivery waits for a *reachable* target,
  and none may be added.

## Per-target ordering

FIFO per target is guaranteed and is defined precisely as **mailbox-position
linearization** — not request order. Admission fixes an entry's position under
the same lock that reserves its quota, so two concurrent sends are ordered
against each other at the one point that can order them, and `peek` returns the
head run in that order.

Mail and raw share one order because both occupy positions in one mailbox.
`peek`'s own contract is what makes raw a barrier structurally rather than by a
separate rule: a raw entry at the head is always returned alone, and mail behind
an unacknowledged raw entry is never returned.

Target-side ordering within one generation follows from the single serial
executor rather than from any additional wait: that executor's own write calls
are sequential, so it cannot begin one while a preceding one it issued is still
in flight. Across a generation replacement, ordering safety is established before
the replacement is admitted at all — a positive `GenerationFence` verdict is
required first, so by the time a replacement's executor calls its first `peek`,
any effect the outgoing generation might still have produced has been positively
observed to have ceased.

## Known gaps

Carried forward from design review as things the current implementation does not
handle. They are limitations, not bugs to be discovered later.

1. **Readiness-to-write TOCTOU, narrowed but not eliminated.** The executor that
   reads readiness is the one that writes, so nothing can intervene between the
   two — which is what closed the relay-side window the push model had. What
   remains is inherent: a target can stop draining in the instant after its own
   transport observed it ready, and no observation can exclude that.

2. **Tmux `drain_invocation_pipes` parks in `read_to_end`** while pipes remain
   open (`tmux/pane.rs`), which is an unbounded wait distinct from the reap loop.
   Any execution-completion predicate must cover both.

3. **Tmux client death is not established as a server-side no-effect cut.**
   Killing an owned tmux client stops further client writes, but whether the tmux
   server can still apply an already-accepted command is not determinable from
   the current source. A fence verdict for Tmux inherits this gap.

4. **Crash recovery is out of scope.** All guarantees hold for a surviving relay
   process and graceful shutdown only. Nothing persists across a process
   boundary.

5. *(Closed.)* `agentmux:issues/relay/62` now has a regression test again —
   `pty_delivery_writes_every_member_of_a_partitioned_group` in
   `tests/unit/pty_transport.rs`, behind the `pty` feature. It asserts on the
   bytes the writer received rather than on outcomes, because a resolved member
   with no bytes behind it was the defect's signature.
