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

### `peek`/`declare`/`ack` reuse the admission guard's ledger, not a new lock

- Decision: `peek`, `declare`, and `ack` are new entry points into the
  existing `AdmissionLedger` (`src/relay/delivery/admission.rs`), not a
  parallel data structure. `declare` performs the packing-unit-binding half
  of what `authorize_batch` did (create the guard, bind it to a
  `PackingUnitId`, for an exact contiguous range); `ack` performs the
  evidence-recording and terminal-transition half the guard's completion
  path did: evidence recording, cursor advancement, quota release, and
  terminalization.
- Why: the guard's own justification for a single relay-owned lock —
  "a keyed map plus a compare-and-set is not sufficient on its own, because
  it cannot observe a detached thread, a worker-task panic, a collector
  panic, or a generation replacement" — applies identically to `declare` and
  `ack`. Reusing the lock rather than inventing new ones is what makes the
  revocation resolution below a composition of existing guarantees instead
  of new machinery.

### Pre-write declaration restores relay-visible submission bookkeeping

**Found during RG review of the first draft, not part of the original
`ideas/21` gap list; recorded here because it changes the operation set.**
The first draft of this proposal moved *all* packing-unit binding to `ack`,
reasoning it was "invoked from `ack` instead of from a push-completion
callback." That collapsed two things the push model kept separate: the
push model declared a partition (via `PartitionSink`) **before** any
target-side effect, and recorded evidence **after** one, via a completion
callback. Binding everything at `ack` alone reproduces only the second
half, and loses the first.

The consequence is real, not cosmetic. A delivery-loop executor that writes
an entry and then panics or hangs before calling `ack` is, at the relay,
indistinguishable from an executor that never touched the entry — there is
no relay-visible record of the attempt. That breaks three things at once:
the execution watchdog (`[delivery].submission-timeout-ms`) was specified
as "anchored at the point a packing unit's write begins," which is not an
event the relay can observe under an ack-only model; shutdown cannot
correctly choose between `dropped_on_shutdown` (nothing happened) and
guard-evidence-order resolution (something may have); and a replacement
generation cannot tell whether re-serving an unacked entry risks a genuine
double-write race or is simply re-serving something truly untouched.

- Decision: reinstate a `declare(target, generation_id, through_seq)` call,
  made before any write is attempted, that binds a `PackingUnitId` to an
  exact contiguous range starting at the cursor. This is the pull model's
  relocation of `PartitionSink`'s pre-effect declaration — the same
  bookkeeping, moved to the new call boundary, not new bookkeeping.
- Why this is not the rejected `claim`/lease step: the rejected step's
  entire objection was about **exclusion** — it stalled a second consumer
  and needed a lease to do it safely, and `ideas/21`'s own rejection
  reasoning is explicit that a lease "borrows its safety from cessation
  observation" and "adds a stall mode without adding a guarantee." `declare`
  grants no exclusivity: it never blocks a second caller, because the
  single-active-generation and single-serial-executor rules already
  guarantee there is no second caller to exclude. It is pure bookkeeping —
  a relay-visible marker between "peeked" and "acked" — with no gating
  semantics at all. Un-rejecting the lease would mean reopening whether a
  second consumer must wait; this reopens nothing, because there was never
  a second consumer to make wait.
- Consequence for `ack`: since a `declare` already names the exact range and
  mints the `PackingUnitId`, `ack` no longer needs (or is permitted to
  accept) a free-form `through_seq`. It names the `packing_unit_id` instead,
  and the relay advances the cursor by exactly the range `declare` already
  recorded. This closes a second problem the first draft's free-form
  `through_seq` left open (see the next decision).
- Consequence for the watchdog: `submission-timeout-ms` is now anchored at
  `declare` acceptance, a relay-observed event, rather than at an inferred
  "write begins" point the relay could never actually see.
- Consequence for shutdown and replacement: an **undeclared** entry is
  provably untouched, so `dropped_on_shutdown` and free re-service to a
  replacement generation are both sound for it. A **declared** entry is
  provably not-provably-untouched, so it always resolves through the
  guard's evidence order and is never re-served or given
  `dropped_on_shutdown` — see `In-Process Delivery Recovery Scope`.

**At most one declared-and-unacked unit per target, found necessary during
RG's second review of `declare` itself.** The first version of `declare`
validated only generation, cursor-position, contiguity, and bounds — it did
not check whether the named entries were *already* declared. Because the
cursor advances only at `ack`, two `declare` calls for the identical
unacked range both pass that validation unchanged (neither has been acked,
so cursor-plus-one is the same for both), minting two distinct
`PackingUnitId`s bound to the same entries: two guards for one member,
which breaks the uniqueness `Delivery Guard and Acknowledgment
Terminalization` requires (a member resolving through two independent
guards has no single terminal transition).

- Decision: `declare` is rejected outright whenever the target already has
  an outstanding declared-and-unacked unit, regardless of what range the
  new call names — not narrowed to an overlap check. A transport must fully
  resolve (ack) one declared unit before declaring another.
- Why the stricter, simpler rule over a narrower same-range or overlap
  check: the single-serial-executor invariant already means a
  well-behaved transport never has two units in flight at once, so nothing
  is lost operationally. An overlap-only check would still permit two
  *non-overlapping* declarations to be outstanding simultaneously, which
  adds a state space (multiple concurrent guards per target) that buys
  nothing, since the executor could not act on both at once anyway. The
  total-order rule collapses that state space to one, which is easier to
  reason about and easier to validate.
- Consequence: the earlier framing of partial acknowledgment as
  "declare several smaller units from one peek and ack each
  independently" is retired. Partial acknowledgment remains ordinary, but
  as a *sequence* of fully-resolved units (declare, write, ack; declare,
  write, ack; ...) rather than several outstanding at once.

### Gap 1 (with P0 #2 folded in) — Revocation serialized against in-flight declaration and acknowledgment

The requirement was already locked by `ideas/21`: an ack must be processed
only while its generation is active, or an already-received ack can commit
after a replacement is admitted. RG's review additionally found that a
free-form `ack(through_seq)` was, on its own, an unconstrained destructive
capability: a caller holding a valid generation binding could `ack` an
arbitrary cursor position it never peeked, wrote, or declared, silently
dropping real messages and releasing their quota. The mechanism below
closes both at once, because they share the same root fix.

- `declare(target, generation_id, through_seq)` and
  `ack(target, generation_id, packing_unit_id, evidence)` both carry the
  `generation_id` the calling consumer bound to.
- **`declare` is validated, not trusted.** The relay rejects a `declare`
  whose named range does not start exactly at the current cursor plus one,
  is not contiguous, or extends past the mailbox's actual contents. Only a
  validated `declare` mints a `PackingUnitId` and binds it. This is what
  makes `ack` safe despite naming only an ID rather than a range: the range
  an `ack` can ever affect was already constrained, contiguously and from
  the cursor, at declare time — there is no path from "valid generation
  binding" to "arbitrary cursor advancement" left, because `ack` cannot
  reference a range that was never legitimately declared.
- Applying a `declare` or an `ack` — binding a unit, or recording evidence
  and advancing the cursor — happens entirely under the ledger's single
  lock, and the **first** thing that critical section does for either call
  is compare the supplied `generation_id` to `active_generation_id`. A
  mismatch rejects the call as stale; a stale `declare`'s entries stay
  `queued` and undeclared, and a stale `ack`'s unit is simply not found
  outstanding under the current generation and is rejected.
- Admitting a replacement generation is symmetric and uses the same lock:
  the relay first drives the existing `GenerationFence` five-step mechanism
  (`transport-abstraction`, unchanged by this proposal) to a **positive**
  verdict for the old generation, and only then, still under the ledger
  lock, flips `active_generation_id`.
- Because all three operations take the same lock, and the flip only
  happens after positive fence observation, there is no window in which a
  `declare` or `ack` that has already passed the generation check inside
  the critical section can be followed by a flip before that same critical
  section releases the lock — they are mutually exclusive by construction,
  not by timing.
- Why this needs no new synchronization primitive: the fence already proves
  "old execution ceased" before admitting a replacement is even attempted;
  the lock already exists and already serializes exactly this class of
  event (guard terminalization, generation replacement) for the push model.
  Extending its coverage to `declare` and `ack` composes things that already
  exist for adjacent purposes, rather than adding new ones.
- Adversarial review should target this mechanism specifically, since
  `ideas/21` flags the revocation half as the one gap whose underlying
  *requirement* is already non-negotiable — only the mechanism is new, and
  a subtle ordering mistake here (checking the generation outside the lock,
  flipping it before the fence verdict, or validating `declare`'s range
  against something other than the live cursor) reintroduces exactly the
  hazard the requirement forbids.

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
  `startup` and living for the transport instance's lifetime, which calls
  the relay's `peek`/`declare`/`ack` entry points directly in-process.
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

### One serial delivery executor per transport instance — in-process scope only

- Decision, narrowed from `ideas/21` at RG's request: this proposal
  specifies **only** that one transport instance maintains exactly one
  serial delivery executor for its lifetime. `ideas/21`'s own text frames
  "reconnect" (a relay reattaching to a still-live transport process across
  a dropped and re-established network connection) as the scenario this
  invariant is *for*, but reconnect itself — connection drop, connection
  swap under a live executor, distinguishing reconnect from a genuinely new
  process — is `ideas/transports/5`'s subject once transports move
  out-of-process, and this proposal does not specify it.
- Why narrowed here rather than left as forward-looking normative text: the
  first draft's spec delta wrote `GenerationFence`-adjacent scenarios about
  a "transport instance's connection to the relay" dropping and
  reconnecting, as SHALL requirements with scenarios — but in-process,
  there is no connection to drop in the first place; a delivery-loop
  executor calls the relay's entry points as ordinary function calls. RG
  correctly flagged this as scope leakage: normative language describing
  behavior that literally cannot occur under this proposal's own stated
  scope. The single-executor invariant itself stays, because it is doing
  real work even in-process (it is what rules out a transport spawning a
  second concurrent writer for one target), but the connection-oriented
  framing around it does not belong in this proposal's spec delta.
- Deferred to `ideas/transports/5`: what "reconnect" means once a
  transport-host process exists, how the relay tells a reconnect from a
  replacement, and the connection-swap-under-a-live-executor mechanics
  `ideas/21` sketches. That note's own text already frames this
  correctly — flag it as inherited context for whoever picks up that
  proposal, not as something this one already committed to.

### Consumer-generation identifiers are a monotonic, non-reused epoch

- Decision, added at RG's request: `active_generation_id` values are drawn
  from a monotonically increasing sequence, per target identity, that is
  never reused — including across a full teardown (worker-registry entry
  reaped, mailbox and cursor state cleaned up per `Mailbox Retention and
  Quota Bounds`) and later recreation of a session under the same name.
- Why: without this, a stale `generation_id` retained by some errant caller
  after a target's full teardown-and-recreation could theoretically collide
  with a freshly assigned id for the "new" target, since nothing previously
  said generation identifiers could not be reused or reset to a default
  starting value per fresh target instance. A monotonic, never-reused
  sequence makes any stale reference guaranteed non-matching rather than
  possibly colliding — the same defense-in-depth reasoning `PackingUnitId`
  already uses (`PackingUnitId::mint()`), extended to this identifier.
- Scope: this is an identity-uniqueness guarantee, not a durability one. The
  sequence itself need not survive a relay crash — `Mailbox Persistence and
  Pruning`'s existing non-durability decision is unaffected — it only needs
  to never repeat a value within one relay process's lifetime for a given
  target identity, which an in-memory monotonic counter provides.

### Every live requirement referencing deleted vocabulary needed its own delta

**Found during EG review of the first revision, then extended by a
self-audit.** EG caught that four *live* `transport-abstraction`
requirements this proposal never touched — `Synchronous Delivery
Completion`, `Transport Module Boundaries`, `Worker Readiness Interface`,
`Positive Activity Signal` — still assume the deleted push model
(`is_ready_for_handover` as a relay-read level, `Pending`/`Authorized`,
authorized batches) and would contradict this proposal's own deltas once
synced. A grep sweep of every live occurrence of `is_ready_for_handover`,
`authorize_batch`, `HandoverWindow`, `` `Pending` ``, and `` `Authorized` ``
across `transport-abstraction` and `transport-contracts` found three more
EG's targeted read had not named: `Pty Transport Implementation`,
`Transport Generation Fencing and Termination Authority` (one stale word:
"Authorized entries" → "declared entries"; its mechanism is otherwise
reused verbatim and unaffected), `Transport Health as a Separate Axis`, and
`ACP Transport Error Code`, plus `Pty Prompt Probe and Look Shall Not Block
a Tokio Worker Thread`.
- Decision: a targeted grep for the deleted vocabulary's exact tokens
  across the full live spec text, not just the requirements a reviewer or
  the original design note happened to name, is now part of this
  proposal's own completeness bar. The sweep found real gaps a
  requirement-by-requirement read had missed twice (once by the author,
  once by a reviewer working from a partial pass).
- A genuine new finding surfaced by the sweep, not just cross-reference
  cleanup: `ACP Transport Error Code`'s live text specifies that an
  undeclared (`Pending`) member resolves `not_submitted` **through the
  guard's evidence order** on definite teardown. That contradicts this
  proposal's own "undeclared members never reach the guard" design — an
  undeclared entry has no guard to route through. Resolved by specifying
  direct relay-side resolution for this one case (teardown/sustained-
  unreachable), serialized under the same lock as `declare`/`ack`/
  replacement so it cannot race a live generation's declaration. See
  `Mailbox Ordering and Cursor Lifecycle`'s new teardown scenarios and the
  extended `Revocation Serialized Against In-Flight Submission Bookkeeping`
  requirement.
- Also restored: the live cross-target contention rationale (tmux socket
  sharing, ACP's shared blocking pool, write-seam occupancy) that the first
  revision's rewrite of `Async Queue Lifecycle and Ordering` dropped when
  renaming it to `Mailbox Ordering and Cursor Lifecycle`. EG flagged this
  as "polish but load-bearing" — it is the actual argument against
  reintroducing cross-target scheduling, not decoration, so trimming it
  silently would have weakened the requirement's own reasoning.

### What is reused unchanged

- `GenerationFence` and its five-step cessation-observation state machine
  (`transport-abstraction`'s "Transport Generation Fencing and Termination
  Authority") — the mechanism that proves "old execution ceased" for
  transport-lifecycle replacement is reused as-is for consumer-generation
  replacement. The delta against that requirement is a single-word
  terminology fix in its own opening sentence (`Authorized` → `declared`,
  found in the sync sweep below); the five-step mechanism itself is
  untouched.
- `SubmissionEvidence` and `PackingUnitId` — these describe
  transport-submission fallibility, which is orthogonal to push vs. pull.
  What moved is *when* binding happens (at `declare`, restoring the
  pre-effect timing the push model had) and *when* evidence is recorded (at
  `ack`) — the vocabulary itself is unchanged.
- The execution watchdog and shutdown-budget nesting — both describe
  bounding the relay's own supervised execution, not the wait for a target.
  The watchdog's anchor point moved from an unobservable "write begins"
  point to `declare` acceptance (a relay-observed event), which is a
  precision fix, not a behavior change to what it bounds.
- `look` and `choice-decisions` — confirmed independent of the deleted
  machinery; no delta proposed against either.
