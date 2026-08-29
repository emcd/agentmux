## Context

`establish-delivery-commit-contract` (archived, 104/104 tasks) built the
current push-model delivery pipeline: `HandoverWindow` batch sizing, a
held-member slot, `is_ready_for_handover`-gated `authorize_batch`, and the
`Pending`/`Authorized`/`Terminal` guard. It works, and it demonstrated a real
defect in doing so: the relay cannot correctly size or time a handover for a
consumer whose internal state it does not observe. ACP's `Busy`-after-one-
envelope refusal and Tmux's timing-decided packing-unit membership are two
independent symptoms of the same root cause, confirmed against
`src/acp/transport.rs` and `src/tmux/transport.rs`.

`agentmux:ideas/21` redesigned delivery around a pull model over six AuxBE
review passes, with the design substantially simplified by operator pushback
each round (a `claim`/lease step, byte-budgeted round-robin scheduling, and a
Pty wedge-content classifier were each proposed, reviewed, and withdrawn in
that arc; see "Non-Goals" and the source note's own "Considered and rejected"
section). The operator authorized converting it to this proposal on
2026-08-29, with an explicit instruction to resolve its four remaining named
gaps normatively during drafting rather than deferring them past approval.

This design.md exists to state those resolutions as decisions, with the
reasoning that makes each one accountable to a reviewer, and to record why
several existing mechanisms are reused rather than rebuilt.

## Goals

- Replace relay-side batch/readiness inference with relay-side custody plus
  transport-side pull, so the relay never again guesses at consumer-internal
  state.
- Preserve every delivery guarantee that does not depend on the deleted
  mechanism: at-most-one-relay-authorized-injection-attempt (now: at-most-one
  acknowledged write per entry per generation), per-target FIFO with raw as a
  barrier, uniqueness of terminal resolution, the execution watchdog and
  generation fence, and the existing crash-recovery scope limitation.
- Resolve all four gaps `ideas/21` left open, with a mechanism, not just a
  restated requirement.
- Keep the in-process implementation IPC-shaped, so `ideas/transports/5`
  inherits a real seam later instead of a second redesign.

## Non-Goals

- No decoupled transport-host processes or wire protocol. In-process function
  calls implement `peek`/`ack` in this proposal; only the type boundary (the
  neutral crate) is IPC-shaped.
- No `claim`/lease step between `peek` and write. Considered and rejected
  across three revisions of `ideas/21` — see that note's "Considered and
  rejected" section. The core argument: a lease's safety is borrowed entirely
  from cessation observation (the generation fence), so it adds a stall mode
  without adding a guarantee, and the no-partial-ack rule it required was
  self-inflicted rather than load-bearing.
- No relay-side token-budget accounting. Token count is a property of an
  entry as rendered by a specific transport, not a property of the entry
  itself; a relay evaluating it would pretend to knowledge it does not have.
  `peek` bounds stay entry-count and canonical-bytes, both relay-evaluable.
- No steering operation (out-of-band raw input that overtakes queued mail).
  Named and deliberately deferred by `ideas/21` as the opposite ordering
  guarantee from `raww`; building it now would overload one operation with
  two incompatible contracts.
- No dedup of at-least-once duplicates.

## Decisions

### The queue-entry state machine collapses to two states

- Decision: an admitted entry is `queued` until an `ack` advances the cursor
  past it, at which point it is `terminal`. There is no `Authorized`
  state and no separate authorization event.
- Why: `Authorized` existed to mark the point past which the relay committed
  to one supervised submission executor and stopped treating the entry as
  reclaimable. Under the pull model that commitment happens at `ack`, not
  before — the relay never invokes anything; the transport calls in. Keeping
  a three-state machine would model a transition that no longer occurs.
- Consequence: `authorize_batch`'s all-or-none batch semantics do not survive
  in any form, because there is no batch to authorize. What a transport peeks
  and what it acks are its own choice, bounded only by FIFO order and the
  raw-singleton rule.

### `peek`/`ack` reuse the admission guard's ledger, not a new lock

- Decision: `peek` and `ack` are new entry points into the existing
  `AdmissionLedger` (`src/relay/delivery/admission.rs`), not a parallel data
  structure. `ack` performs, under the ledger's single lock, exactly the
  operations `authorize_batch` plus the guard's terminal transition perform
  today: evidence recording, packing-unit binding, cursor advancement, quota
  release, and terminalization.
- Why: the guard's own justification for a single relay-owned lock —
  "a keyed map plus a compare-and-set is not sufficient on its own, because
  it cannot observe a detached thread, a worker-task panic, a collector
  panic, or a generation replacement" — applies identically to `ack`. Reusing
  the lock rather than inventing a second one is what makes the revocation
  resolution below a composition of existing guarantees instead of new
  machinery.

### Gap 1 — Revocation serialized against in-flight acknowledgment

The requirement was already locked by `ideas/21`: an ack must be processed
only while its generation is active, or an already-received ack can commit
after a replacement is admitted. The mechanism:

- `ack(target, generation_id, through_seq, evidence)` carries the
  `generation_id` the calling consumer bound to at its last successful `peek`
  or connection establishment.
- Applying an `ack` — recording evidence, advancing the cursor, releasing
  quota, terminalizing members — happens entirely under the ledger's single
  lock, and the **first** thing that critical section does is compare the
  supplied `generation_id` to `active_generation_id`. A mismatch rejects the
  `ack` as stale; the targeted entries are left `queued` exactly as if the
  write had never happened, and they are re-served whole to whatever
  generation is current on its next `peek`. This is safe under at-least-once:
  the write already reached the target, so the outcome is a possible
  duplicate delivery, the accepted tradeoff, not a correctness violation.
- Admitting a replacement generation is symmetric and uses the same lock:
  the relay first drives the existing `GenerationFence` five-step mechanism
  (`transport-abstraction`, unchanged by this proposal) to a **positive**
  verdict for the old generation, and only then, still under the ledger
  lock, flips `active_generation_id`.
- Because both operations take the same lock, and the flip only happens
  after positive fence observation, there is no window in which an `ack`
  that has already passed the generation check inside the critical section
  can be followed by a flip before that same critical section releases the
  lock — the two are mutually exclusive by construction, not by timing.
- Why this needs no new synchronization primitive: the fence already proves
  "old execution ceased" before admitting a replacement is even attempted;
  the lock already exists and already serializes exactly this class of
  event (guard terminalization, generation replacement) for the push model.
  Extending its coverage to `ack` composes two things that already exist for
  adjacent purposes, rather than adding a third.
- Adversarial review should target this mechanism specifically, since
  `ideas/21` flags it as the one gap whose underlying *requirement* is
  already non-negotiable — only the mechanism is new, and a subtle ordering
  mistake here (checking the generation outside the lock, or flipping it
  before the fence verdict) reintroduces exactly the hazard the requirement
  forbids.

### Gap 2 — Doorbell durability across restart

- Decision: the doorbell is deliberately non-durable. It is rebuilt fresh per
  transport generation at construction time, exactly like the existing
  `readiness_changed: Arc<Notify>` it replaces
  (`src/relay/delivery/dispatch/worker.rs:210,716-718`), and correctness
  never depends on any doorbell notification arriving.
- Why: the doorbell's own contract already states this — "losing a doorbell
  loses nothing; it only delays, and a periodic poll is the backstop." A
  durability mechanism would be solving a problem the contract already
  excludes by construction. `transport-abstraction`'s existing scenario "A
  transport signals upward through an injected closure" is the identical
  pattern already specified for the readiness edge-hint this doorbell
  succeeds; no new requirement shape is needed, only its reuse for a
  different event (mailbox non-empty rather than target-readiness-changed).
- What does survive a generation replacement: the mailbox contents and
  cursor themselves, which live in the relay's `AdmissionLedger`, independent
  of any transport generation's lifetime. A new generation's first `peek`
  sees exactly what an old generation would have, doorbell or not.

### Gap 3 — Policy change while entries remain queued

- Decision: authorization policy is evaluated once, at admission, and the
  decision is a property of the admitted entry from then on. A policy change
  is prospective only: it governs future admissions and has no effect on
  entries already sitting in a mailbox, whether or not they have been
  peeked.
- Why: the alternative — re-evaluating policy at peek or ack time — would
  mean a sender who was told `queued` at admission could have that message
  silently dropped or blocked by a policy change it has no visibility into,
  which contradicts the existing terminal-outcome-receipt contract (a
  non-delivered outcome always produces a receipt naming a reason). Snapshot-
  at-admission also matches how admission quota reservation already works:
  reserved once, at admission, and never re-evaluated against a config
  change until release. No new machinery is needed — an admitted entry
  simply carries no policy reference to re-check.
- Scope: this governs entries already admitted. It says nothing about
  whether a *new* send should be evaluated against old or new policy at
  admission time, which is an ordinary request-time authorization question
  already covered by `relay-routing-layer`'s authorization stage and is not
  part of this capability.

### Gap 4 — Mailbox persistence format and pruning

- Decision: the mailbox stays in-memory and non-durable, exactly as the
  current pending queue is specified today (`delivery-quiescence`'s existing
  "the queue SHALL be non-durable"). No new persistence format is
  introduced. Growth is bounded by the existing per-target and relay-global
  admission quota (envelope count and canonical bytes) — the same mechanism
  that bounds the current `Pending` backlog — and no new TTL or pruning
  mechanism is added.
- Why: `litrpg`'s reference design prunes by TTL and drops identities with
  empty mailboxes, which prompted the question, but nothing in this
  project's admission model changed shape in a way that requires it. A
  mailbox with unacked entries is already bounded by quota; a mailbox that
  has emptied (every entry terminal) is already eligible for cleanup exactly
  when its worker-registry entry is reaped on generation teardown without
  replacement — an existing lifecycle event
  (`src/relay/delivery/async_worker/registry.rs`), not a new one. Building a
  separate pruning pass would duplicate a cleanup trigger that already
  exists for a different but coincident reason.
- What remains explicitly out of scope: durability across a relay crash.
  `delivery-quiescence`'s existing "In-Process Delivery Recovery Scope"
  requirement already limits guarantees to a surviving process; this
  proposal restates that limitation in mailbox vocabulary rather than
  widening it.

### The `Transport` trait inverts its delivery methods

- Decision: `mailw` and `raww` are removed from the `Transport` trait
  entirely. The relay no longer calls into a transport to deliver. Instead,
  each transport owns one serial delivery-loop executor, spawned at
  `startup` and living for the transport instance's lifetime (independent of
  any relay connection to it, per the single-serial-executor decision
  below), which calls the relay's `peek`/`ack` entry points directly
  in-process.
- Why: this is the direction reversal the design describes as "relay→
  transport for reads [`look`], transport→relay for peek/ack" and is the
  entire point of the neutral crate — both call directions need to coexist
  without either side importing the other's concrete types.
- What is unaffected: how a transport renders and writes once it has
  something to write. Tmux's token-budget packing, ACP's `session/prompt`
  framing, Pty's master-write — all specified today in `transport-contracts`
  and `transport-abstraction` — are called from inside the transport's own
  delivery loop instead of from a relay-invoked `mailw`, with no change to
  their own contracts.
- `raww` survives as a relay-inbound API name (a caller still invokes
  `raww` to submit raw input), but its effect is now to enqueue a raw-kind
  mailbox entry rather than to push directly into a transport. The transport
  discovers it exactly as it discovers mail — by peeking it, as a singleton.

### One serial delivery executor per transport instance, independent of reconnect

- Decision, reused verbatim from `ideas/21`: a transport instance maintains
  exactly one serial delivery executor for its lifetime. Reconnect (a relay
  reattaching to the same live transport process, relevant once
  `ideas/transports/5` lands) swaps the connection underneath a live
  executor; it never starts a second poll/write loop for the same
  generation.
- Why restated here: this is what makes "same-generation reconnect" safe
  without invoking revocation at all. A transport that spawned an executor
  per inbound connection would violate this invariant, and — per the source
  note — "the alternative is the fallback if any implementation cannot hold
  the single-executor invariant. It should not be needed." This proposal's
  in-process implementation trivially satisfies it (there is no reconnect
  in-process; the executor is a task or thread owned for the transport
  instance's life), which is exactly the "in-process first, IPC boundary
  designed in" posture.

### What is reused unchanged

- `GenerationFence` and its five-step cessation-observation state machine
  (`transport-abstraction`'s "Transport Generation Fencing and Termination
  Authority") — the mechanism that proves "old execution ceased" for
  transport-lifecycle replacement is reused as-is for consumer-generation
  replacement. No delta is proposed against that requirement.
- `SubmissionEvidence`, `PackingUnitId`, and the guard's evidence-order
  resolution (immutable evidence → derive outcome; never bound → 
  `not_submitted`; otherwise `submission_unknown`) — these describe
  transport-submission fallibility, which is orthogonal to push vs. pull and
  needs no change beyond being invoked from `ack` instead of from a
  push-completion callback.
- The execution watchdog (`submission-timeout-ms` anchored per attempt) and
  shutdown-budget nesting — both describe bounding the relay's own
  supervised execution, not the wait for a target, and neither depends on
  the deleted authorization step.
- `look` and `choice-decisions` — confirmed independent of the deleted
  machinery; no delta proposed against either.
