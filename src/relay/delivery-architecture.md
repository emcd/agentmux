# Relay Delivery Architecture

How a message travels from a `send` call to a terminal outcome, and where the
relay's responsibilities end and a transport's begin.

Companion document: [`delivery-decisions.md`](delivery-decisions.md) records
*why* the shape is this way. This file records *what* the shape is.

**Transitional: the diagrams below describe the push model, which is the path
still in production.** The relay-side mailbox of the pull model that replaces it
— `peek`, `declare`, `ack`, and the two-state entry lifecycle — is implemented
and described in [`README.md`](README.md), but no transport drives it yet, so
every delivery still travels the path drawn here.

One part of it is nevertheless live. Each admitted entry's payload is now built
at the delivery worker's task intake and placed in its target's mailbox, and the
push path writes *that* stored artifact rather than rendering one of its own — so
the envelope a future executor will be handed is the envelope production delivers
today. The mailbox is drained by the terminal transition, which retires each
position as it resolves it, because nothing acknowledges yet. What has not moved
is who decides when to write, which is the whole of the remaining cutover.

Read the entry states below
(`Pending`, `Authorized`, `Terminal`) as the states of that path. They are
collapsed to `Queued` and `Terminal`, and authorization is replaced by
declaration, when the transports' delivery-loop executors land; these diagrams
are rewritten in the same change rather than tracking a model that is half
migrated. See D11 in `delivery-decisions.md` for what survives the collapse.

Diagrams are ASCII rather than Mermaid so they read identically in a terminal,
an editor, and a web view. The repository uses no Mermaid elsewhere.

## Status legend

The delivery contract is mid-implementation under the
`establish-delivery-commit-contract` OpenSpec change. Everything below is marked,
because the difference between "specified" and "built" has already caused one
wrong architectural conclusion in review:

- **[built]** — implemented on `master` and exercised by tests
- **[partial]** — some transports or some paths only; the gap is named
- **[specified]** — in the spec deltas, no implementation yet

## The four events

The contract's central move is separating four things that delivery code
historically collapsed into one. Each has a different owner and a different
truth condition.

```
   ADMISSION            AUTHORIZATION           SUBMISSION            RESOLUTION
   [built]              [built]                 [partial]             [built]

   accept into queue    Pending -> Authorized   a packing unit        each member
   reserve quota        the linearization       produces a            resolves once,
   return "queued"      point                   target-side effect    from evidence

   relay owns           relay owns              transport owns        relay owns
   synchronous          relay-local             observed, reported    guard + CAS
                        never reclaims after
```

The rule that makes this worth the machinery: **before authorization the relay
may truthfully report non-delivery, because nothing was written. After
authorization it may not, because bytes may already be moving.** Reporting from
position rather than from evidence is the defect this whole contract exists to
remove (`agentmux:issues/relay/61`, `agentmux:issues/relay/62`).

## Component layout

```
  caller (CLI / MCP / TUI / peer relay)
     |
     |  send
     v
  +--------------------------------------------------------------+
  |  ADMISSION                          relay/delivery/admission.rs |
  |                                                              |
  |  - reject Pubsub synchronously            [built]            |
  |  - reject oversized canonical payload     [built]            |
  |  - reserve per-target + global quota      [built]            |
  |    (count and bytes, atomically)                             |
  |                                                              |
  |  returns "queued" -- NOT "delivered"                         |
  +--------------------------------------------------------------+
     |
     |  enqueue (unbounded mpsc, one channel per target)
     v
  +--------------------------------------------------------------+
  |  PENDING QUEUE                                               |
  |                                                              |
  |  Entry states: Pending | Authorized | Terminal    [built]    |
  |  relay/delivery/guard.rs :: QueueEntryState                  |
  |                                                              |
  |  A Pending entry holds its quota reservation. It leaves      |
  |  Pending ONLY by:                                            |
  |    - authorization                                           |
  |    - positively observed transport teardown                  |
  |    - sustained unreachability past the dwell                 |
  |    - graceful shutdown                                       |
  |  No elapsed duration resolves a Pending entry whose target   |
  |  is reachable. This is deliberate; see delivery-decisions.md |
  +--------------------------------------------------------------+
     |
     v
  +--------------------------------------------------------------+
  |  DISPATCH WORKER            relay/delivery/dispatch/worker.rs |
  |  one tokio task per target                                   |
  |                                                              |
  |  TWO GATES, both required, neither substituting for the      |
  |  other (worker.rs:419-440):                                  |
  |                                                              |
  |    health()                 -> Healthy | Unreachable{since}  |
  |       Unreachable past dwell  -> resolve not_submitted       |
  |       Unreachable within dwell-> hold Pending                |
  |                                                              |
  |    is_ready_for_handover()  -> bool                          |
  |       false                   -> hold Pending (indefinitely) |
  |                                                              |
  |  Health says WHETHER a handover is possible.                 |
  |  Readiness says WHEN it is useful.                           |
  |                                                              |
  |  Both reads are ADVISORY -- they can go stale between the    |
  |  check and the write. See "Known gaps" below.                |
  +--------------------------------------------------------------+
     |
     |  authorize_member()   <-- THE LINEARIZATION POINT (:448)
     |  creates the guard in the same atomic operation
     v
  +--------------------------------------------------------------+
  |  TRANSPORT SEAM       mailw / raww -- NON-BLOCKING  [built]  |
  |                                                              |
  |  Every transport enqueues onto its own ordered channel via   |
  |  try_send and returns an OutcomeFuture immediately. A        |
  |  transport's blocking IO never pins the dispatch worker.     |
  +--------------------------------------------------------------+
     |
     v
  +--------------------------------------------------------------+
  |  TRANSPORT-INTERNAL DELIVERY TASK      (per transport)       |
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
     |  OutcomeFuture resolves, collected in a JoinSet
     v
  +--------------------------------------------------------------+
  |  RESOLUTION                                       [built]    |
  |                                                              |
  |  Single terminal CAS. Quota releases HERE and nowhere else.  |
  |                                                              |
  |  Evidence order:                                             |
  |    unit record if present                                    |
  |    else not_submitted   (member never bound to a unit)       |
  |    else submission_unknown                                   |
  +--------------------------------------------------------------+
```

## Delivery sequence

The normal path, and the two ways it diverges.

```
 caller        relay              dispatch worker      transport      target
   |             |                      |                  |            |
   |-- send ---->|                      |                  |            |
   |             |                      |                  |            |
   |        [admission]                 |                  |            |
   |        reserve quota               |                  |            |
   |<-- queued --|                      |                  |            |
   |             |                      |                  |            |
   |             |--- enqueue --------->|                  |            |
   |             |                      |                  |            |
   |             |                 [gate 1] health()       |            |
   |             |                      |----------------->|            |
   |             |                      |<-- Healthy ------|            |
   |             |                      |                  |            |
   |             |                 [gate 2] is_ready_for_handover()     |
   |             |                      |----------------->|            |
   |             |                      |<-- true ---------|            |
   |             |                      |                  |            |
   |             |                 [AUTHORIZE]             |            |
   |             |                 guard created           |            |
   |             |                 quota now held by guard |            |
   |             |                      |                  |            |
   |             |                      |-- mailw -------->|            |
   |             |                      |   (try_send,     |            |
   |             |                      |    non-blocking) |            |
   |             |                      |<-- OutcomeFuture-|            |
   |             |                      |                  |            |
   |             |                      |             [partition]       |
   |             |                      |             into packing units|
   |             |                      |                  |            |
   |             |                      |                  |-- write -->|
   |             |                      |                  |            |
   |             |                      |             [record evidence] |
   |             |                      |              Submitted        |
   |             |                      |                  |            |
   |             |                      |<-- outcome ------|            |
   |             |                      |                  |            |
   |             |                 [terminal CAS]          |            |
   |             |                 quota released          |            |
   |             |<-- receipt ----------|                  |            |
   |             |                      |                  |            |
```

### Divergence A — target not ready

```
   |             |                 [gate 2] is_ready_for_handover()     |
   |             |                      |----------------->|            |
   |             |                      |<-- false --------|            |
   |             |                      |                  |            |
   |             |                 entry stays Pending                  |
   |             |                 quota stays reserved                 |
   |             |                 NO timer runs                        |
   |             |                      |                  |            |
   |             |                 (the worker re-reads the level on a  |
   |             |                  poll; a transport MAY additionally   |
   |             |                  invoke a relay-provided wakeup       |
   |             |                  closure to shorten the latency)      |
```

**The poll is the guarantee, not the notification.** `TransportImpl::tmux` takes
an `Option<ReadinessNotifier>` and the contract states the obligation precisely
(`transports/contract.rs:463-468`): the delivery contract does not oblige a
transport to have a notification path, correctness never depends on one, the
level the relay reads is authoritative, and a missing wakeup only defers a
delivery to the next poll. Today only Tmux is wired for the notifier, and its
type is Tmux-specific rather than generic. Treat a wakeup as a latency
optimisation on every transport.

**Subscribe-before-check is implemented, not merely intended.** The worker
creates its `Notify` at `worker.rs:194` — before any level is read and before the
transport that will poke it exists — and passes the notifier during transport
construction at `:370-379`, with the gate reads at `:419-440`. The ordering
therefore makes a change occurring between check and subscription
unrepresentable rather than merely unlikely. The poll below remains the
lost-wakeup backstop.

The sender is told `queued` and hears nothing further until the target becomes
ready. This is the behaviour that replaced the prime/readiness/wedge timers. A
long agent turn produces a backlog of `Pending` entries in the relay queue —
**not** a backlog in the target's pipe, because gate 2 never let the write start.

### Divergence B — target continuously unreachable

```
   |             |                 [gate 1] health()                    |
   |             |                      |----------------->|            |
   |             |                      |<-- Unreachable{since} --------|
   |             |                      |                  |            |
   |             |                 since.elapsed() >= unreachable_dwell?|
   |             |                      |                  |            |
   |             |                 no  -> hold Pending                  |
   |             |                 yes -> resolve not_submitted         |
   |             |                        reason_code:                  |
   |             |                        delivery_target_unreachable   |
   |             |                        quota released                |
```

Any return to `Healthy` resets the dwell, so a transient unreachability resolves
nothing.

## Reporting the partition

The relay hands over one envelope at a time and cannot see what a transport does
with them. ACP coalesces a budget group into one `session/prompt`; Tmux splits a
batch into token-budgeted pastes; Pty writes each member separately. That
partition decides which members share a fate, so the guard has to learn it from
the layer that chose it.

`PartitionSink` (`transports/contract.rs`) is that seam, injected as an
`Arc<dyn PartitionSink>` at transport construction — the same shape as ACP's
`MirrorStateFn` and Pty's `PtyMirrorStateFn`, and for the same reason:
`src/transports` may not import `crate::relay`. The relay's implementation is
`relay/delivery/partition.rs`, which delegates to the admission ledger.

Two calls bracket the write:

```
  declare(&[member_ids]) -> Result<PackingUnitId, PartitionError>
      before the first target-side effect
      binds every proposed member or none, under the ledger lock
      Err obliges the transport to produce NO effect for that unit

  record(unit, evidence)
      once, before member fan-out
      the one record every member of the unit resolves from
```

`declare` refuses rather than binding a subset, and that asymmetry is the point.
Binding *late* costs precision — the evidence order falls back to
`submission_unknown` once a member is bound. Binding *partially* costs
correctness: a member that terminalized a moment earlier would resolve
`not_submitted`, a positive claim that nothing was written, while the transport
went on to write the group it had already composed. A vetoing member therefore
vetoes its whole proposed unit, and its groupmates resolve `not_submitted`
because no effect occurred for them either.

What the seam enforces is everything after `record` returns: there is one record
per unit, so two members of one unit cannot disagree. What it cannot enforce is
the ordering before `declare` — nothing stops a transport side-effecting first
and declaring afterwards. That stays a per-transport boundary test.

Adoption is per-transport. `TransportImpl::reports_own_partition` says which
transports report their own; the relay declares a singleton unit for the rest.
ACP additionally distinguishes *who* declared a turn's unit through `TurnUnit`,
because its raw path reaches the same submission function with a synthetic member
id it cannot declare against.
Raw is relay-declared permanently, in either case: neither `Transport::raww` nor
Pty's `DeliveryCommand::Raw` carries a message id, so no transport can name the
member at its raw write.

| Transport | Unit | Reports its own |
|-----------|------|-----------------|
| Tmux | one token-budget group per paste | yes |
| ACP | one budget group per `session/prompt` | yes |
| Pty | one member per write | yes |
| UI | one member | no — relay declares |

Pty's one-member-per-unit is a commitment rather than a limitation: coalescing
would smear a partial write's evidence across members, since an earlier member's
bytes could land while a later member's did not and a shared unit could only
report one answer for both.

### Members no unit covers

A terminal-outcome receipt bypasses admission — it holds no quota reservation and
no ledger entry — so it belongs to no packing unit and is resolved by its own
outcome sender rather than by the guard. Nothing declares one: not the relay
(`declare_singleton_unit` returns early for an un-admitted task) and not a
transport (each excludes receipt members from its declaration).

This is not an optimisation. Asking the ledger about a receipt returns
`MemberNotBindable`, the *same* answer it gives for a member that already
terminalized, because a winning terminal transition removes the entry and absence
cannot distinguish the two. Under the contract a refused declaration obliges the
transport to produce no effect — so a declared receipt is a dropped receipt, and
the drop is silent, since receipts are relay-originated and nothing downstream
waits on one.

## Generation fencing

A transport *generation* is one instance of a transport plus everything it owns —
child processes, executors, threads. Replacing a generation (respawn) or shutting
down requires proving the old generation stopped, because an un-stopped executor
can still produce a target-side effect attributed to the wrong generation.

Five steps, in `relay/delivery/fence.rs` and the per-transport
`terminate_generation` / `generation_ceased` pair. **[built]**

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
described below. **[built]**

A **positive** verdict tears the old generation down and builds a replacement in
place, from the same source the worker's first generation was built from — so a
target cannot acquire a second transport kind by being fenced. A **negative**
verdict marks the target fail-stopped:
its registry entry is held for the rest of the process's life, which is what makes
a replacement unelectable, and every further send is refused with
`delivery_target_fail_stopped`. Raw needs no separate barrier — `raww` reaches the
target through that same registry lookup. Either verdict resolves every
still-unresolved member through the guard. **[built]**

## The execution watchdog

The one gap the two-condition delivery model (relay shutting down / transport
unhealthy) cannot close: an executor **alive, with a healthy target, stuck in our
own code** — parked in a blocking write whose buffer is full. Neither condition
fires and the member stays `Authorized` forever, holding quota and blocking the
target FIFO.

```
  authorization ──────────────────────────────────► resolution
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
transport resolves its outcome future at the write boundary — ACP after the framed
`session/prompt` write, Tmux at the `inject_literal_text` invocation, Pty at the
buffered `write_all`. Break that property in any transport and the watchdog starts
fencing healthy targets mid-turn. **[built]**

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
  transport's internal delivery task.
- No configuration key bounds how long a delivery waits for a *reachable* target,
  and none may be added.

## Per-target ordering

FIFO per target is guaranteed and is defined precisely as **worker-enqueue
linearization** — not request order, and not admission order. `try_existing_worker`
holds the registry lock across `sender.send`, and the channel is unbounded so the
send cannot block, therefore channel order equals lock-acquisition order.

Mail and raw share one order because both reach the worker through
`enqueue_async_delivery`. A request may reserve admission quota first and still
lose the enqueue race, which is why admission order is not the guarantee.

## Known gaps

Carried forward from design review as things the current implementation does not
handle. They are limitations, not bugs to be discovered later.

1. **Authorization-to-write TOCTOU.** Both gate reads are advisory (the comment
   at `worker.rs:405-408` says so explicitly). A target can stop draining
   immediately after `Available` was read, between gate 2 and the write.

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
