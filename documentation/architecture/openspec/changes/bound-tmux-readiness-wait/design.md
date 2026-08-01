## Context

`quiescence_classify_step` alternates classify and wait: it either resolves
terminally or returns `NeedsWait(deadline)`, and the caller blocks in
`wait_for_change(deadline)`. Termination therefore requires both a finite
deadline and a classifier that can reach a verdict.

The Busy short-circuit (`src/transports/quiescence.rs:413-441`) fires whenever
the activity signal advances between the paired observations and returns ahead
of every terminal check. `transport-abstraction/spec.md:443-449` mandates this:
"While the terminal-output-write signal continues to be reported across
iterations, the classifier SHALL NOT promote the flush group to ANY terminal
classification." For a target that emits output indefinitely without reaching a
prompt — a retry banner, a countdown — that sentence has no exit. The incident
log shows 103 mismatches against 2365 activity ticks across 47 minutes with no
verdict.

The same specification already contains the fix in principle.
`transport-abstraction/spec.md:518-527`, added by
`remove-operator-interaction-delivery-gate`, says a quiescence wait "SHALL
always progress toward one of its terminal classifications" and forbids holding
a flush group in a non-terminal state "on the basis of a rendering signal it
cannot bound." That was written for copy-mode. It applies verbatim to the
activity signal, which is equally unbounded. The gap is that the principle was
stated for one signal and given no enforcement mechanism.

## Goals / Non-Goals

**Goals:**

- Give the existing progress-toward-termination principle a bound on Tmux, and
  generalize it there from rendering signals to every unbounded signal.
- Keep the change surgical: one new orthogonal bound rather than a rework of the
  classifier's internal state.
- Preserve the diagnostic quality of the wedge fast path.

**Non-Goals:**

- Bounding Pty or ACP. See the commitment-point decision below;
  `agentmux:issues/relay/61` carries that work.
- Any new `SendOutcome` variant, any relay-side change, or any reconciliation of
  the eleven specs that carry outcome vocabulary.
- Changing Busy semantics, wedge-counter continuity, or
  `WEDGE_CONSECUTIVE_TICKS`. An earlier draft did; see Decisions.
- Distinguishing "generating tokens" from "animating a non-prompt state" by
  inspecting target bytes.
- Per-message deadlines within a flush group; the group remains the unit.

## Decisions

### Tmux only, because commitment follows the wait there

An earlier draft of this change stated a universal invariant across every
transport. Review established that the invariant cannot be honestly stated
that way yet, because a bound means different things depending on when it
expires relative to the transport's **point of commitment** — the instant after
which the target may already have received the message.

| Transport | Commitment point | Relative to the wait | An expired bound means |
|---|---|---|---|
| Tmux | injection into the pane | after | genuinely not delivered |
| Pty | write to the PTY master | before | delivered, unconfirmed |
| ACP | `client.prompt` returning `Submitted` | before | submitted, uncompleted |

Verified in code: `src/pty/delivery.rs::start_envelope_group` documents that
"All envelopes are written to the PTY master immediately", with the write loop
preceding the quiescence wait; `src/acp/transport.rs` anchors
`prime_started_at` inside the `PromptDispatchOutcome::Submitted` arm.

Only Tmux can report an expired bound as non-delivery without asserting
something it cannot establish. Bounding the others requires an outcome meaning
"unconfirmed", which is a large and separable change. Scoping here to Tmux is
therefore not a convenience: it is the boundary at which the existing outcome
vocabulary stays truthful.

The requirement text names Tmux rather than stating a universal rule with an
exception. "Every wait is bounded, except Pty and ACP" is a sentence a later
reader takes as complete; "the Tmux readiness wait is bounded" is not.

### One orthogonal bound, not a classifier rework

The readiness bound is evaluated on every iteration and nothing may defer it.
Busy otherwise keeps its current behavior in full: it still suppresses terminal
classification for its iteration, and it still resets the wedge counter.

An earlier draft instead forbade Busy from resetting the wedge counter, on the
theory that the reset was part of the defect. Review showed that to be a bug:
the wedge condition requires *continuous* frame-absence, so a counter that
survives an activity burst lets an old wedge start combine with newly settled
content and fire immediately. The reset is what makes wedge continuity correct,
and the defect was never the reset — it was that nothing bounded the loop the
reset lived in.

This also removes the need to re-denominate the wedge counter in elapsed time.
That was proposed to fix Band 1, and Band 1 is already fixed: the bounded
recheck guarantees observations occur on a timer, so the counter advances
without the target changing. The remaining tick problem is textual — the specs
disagree with each other and with the code — and is resolved by editing the text
to match the shipped threshold.

### Bound evaluation is not a positional early return

The bound must not be expressed as "check readiness first and return", because
that contradicts the precedence rule below: a positional early return would
report the readiness outcome in an iteration where a higher-precedence outcome
was available — a delivery the target had just become ready for, a satisfied
wedge predicate, or a prime timeout.

The contract is therefore stated as two steps. First, evaluate every available
outcome — delivery readiness, the wedge predicate, and each elapsed bound.
Second, apply precedence. What the bound
guarantees is that no iteration may return `NeedsWait` once it has elapsed, and
that no returned deadline may exceed it — not that it occupies a particular
branch position.

### Scope, anchor, and precedence

Scope: the entire wait for the flush group, including the pre-quiescence prime
window and any period of continuous activity. Not merely the post-quiescence
readiness wait, which was ambiguous in the previous draft.

Anchor: the same instant as `prime_started_at`, already the delivery task's view
of when the group's wait began. Reusing it avoids a second notion of "when this
started" and keeps coalesced envelopes on the head envelope's clock, consistent
with the existing head-envelope hint rule.

Precedence when both bounds have elapsed in the same iteration: the prime
timeout wins. It is the more specific diagnosis — no observable output at all —
and it is opt-in, so an operator who set it asked for that answer. Otherwise the
earlier bound governs.

### The bound

| Setting | Default | Range |
|---|---|---|
| `[coders.<id>.tmux].readiness-timeout-ms` | `900_000` (15 min) | `30_000`..=`3_600_000` |

The default must exceed the longest plausible agent turn, because a target
mid-turn is legitimately not ready and its message should wait. Observed turns
with heavy tool use run to several minutes; 15 minutes gives roughly three times
that headroom while surfacing a stall well inside the 68 minutes the incident
ran. The lower range bound keeps an operator from configuring a value beneath a
single turn; the upper bound keeps the setting from re-creating an effectively
unbounded wait.

### Terminal taxonomy

A readiness-bound expiry is never a wedge. The wedge verdict has its own
predicate — a wedge-class mismatch repeated across `WEDGE_CONSECUTIVE_TICKS`
consecutive quiescent evaluations — and a frame first observed absent at the
instant the bound elapses has not satisfied it. Reporting `pane_wedged` there
would assert a repeated, settled diagnosis from a single observation, which is
exactly the claim the tick threshold exists to prevent. Since this change
preserves the threshold unchanged, the taxonomy has to respect it.

`pane_wedged` therefore belongs to the wedge path alone, which resolves the
group before the bound is ever consulted. Everything reaching the bound resolves
`Timeout`, with the reason taken from the most recent observation:

| Most recent observation at expiry | Outcome | `reason_code` |
|---|---|---|
| Frame absent, wedge predicate not yet satisfied | `Timeout` | `target_not_ready` |
| Inspected tail empty | `Timeout` | `target_unresponsive` |
| Frame present, cursor off idle column | `Timeout` | `pending_operator_input` |
| Activity advancing | `Timeout` | `target_never_settled` |

Note that wedge-enabled and wedge-disabled collapse to the same row. Whether the
knob is on changes only whether the wedge path could have fired *earlier*; it
does not change what an expiry means, because an expiry reached with the
predicate unsatisfied is the same observation either way.

Two orderings, easily conflated:

- **Reason precedence** within one observation pair, highest first: activity
  advancing, then empty tail, then cursor mismatch, then frame absence. Activity
  ranks first because it is the only signal describing the pair rather than the
  final snapshot.
- **Verdict precedence** when more than one outcome is available in the same
  iteration, highest first: delivery, then the wedge predicate, then the prime
  timeout, then the readiness bound.

Delivery ranking first is the non-obvious one. A target that becomes prompt-ready
in the very iteration the bound expires is delivered to, not timed out: reaching
readiness late is the outcome the wait existed to obtain, and the bound's purpose
is to stop waiting forever rather than to refuse a success already in hand. It
also matches the existing branch order, where the prompt-ready check already
precedes the prime-timeout check, so this extends a rule rather than inventing
one. Busy is unaffected — a prompt-ready observation whose activity advanced is
still deferred, and an elapsed bound then resolves it as `target_never_settled`
rather than granting the match Busy had just denied.

Below delivery, the wedge predicate and prime timeout outrank the readiness
bound as more specific diagnoses, reached only under conditions the readiness
bound does not describe. Wedge cannot conflict with delivery, since it requires
the target not be prompt-ready.

The three states remain mutually exclusive at the moment of terminal
classification, as the existing requirement demands.

## Risks / Trade-offs

- **A long agent turn fails a delivery that would have succeeded** → The
  substantive risk, and why the default is 15 minutes rather than something
  tighter. Mitigated by the per-coder key, by a lower range bound that prevents
  configuring below one turn, and by `target_never_settled` naming the cause so
  an operator can distinguish a short bound from a stuck target. On Tmux the
  failure is non-destructive — injection had not happened, so the sender learns
  the message did not land and can resend. That is what makes a generous bound
  affordable, and it is a Tmux-specific property, not a general one.
- **The default has a moving goalpost** → Turns exceeding 15 minutes are
  atypical today but already occur, and long-horizon agent work is trending
  longer. Any fixed constant separating "working" from "stuck" will drift out of
  calibration, because the transport cannot distinguish the two by inspection —
  time is the only separator available. This design accepts that and optimizes
  for cheap recalibration rather than a durable constant: the bound is
  per-coder, the upper range bound is an hour so the setting does not itself
  become the constraint, and the reason code tells an operator which way to move
  it. Deliberately not mitigated by letting the activity signal extend the
  bound, which is precisely the defect being removed.
- **A shared state machine may activate the bound for Pty by accident** → Tmux
  and Pty drive the same classifier, so a bound plumbed carelessly would change
  Pty behavior this change has not specified and whose outcome vocabulary would
  be wrong. Pty passes no readiness bound, and coverage asserts that a Pty-shaped
  delivery is unaffected.
- **Removing the unbounded path changes behavior for operators who opted out**
  → Both Tmux opt-outs currently mean "wait forever". They are re-scoped to "no
  early verdict". Called out as BREAKING rather than shimmed.
- **A deleted safety claim may read as a removed feature** → The
  operator-interaction prose documents a gate already deleted, so only stale
  text goes. The risk is the inverse: a reader may conclude the composing case
  is still covered, which is how this text misled the Band 1 review.

## Migration Plan

No data or configuration migration. The new key is optional with a default.
Rollback is a revert; no persisted state changes shape.

Sequencing: this change edits `src/transports/quiescence.rs`, which is also
touched by the Band 1 merge and by `agentmux:issues/relay/60`. It follows both,
and its reason-classifier work populates relay/60's diagnostic context type
rather than introducing a parallel path.

## Open Questions

1. Is a fixed constant the right shape at all, given the moving goalpost above?
   A bound derived from the coder's observed turn behavior would recalibrate
   itself, but it needs a history the transport does not keep, and a target
   whose turns are lengthening because it is stuck would raise its own ceiling.
   Recorded as a known limitation of the constant rather than proposed here.
2. Should `pending_operator_input` expiry be reported to the sender differently
   from the other timeout reasons? It is the one case where the target is
   healthy and a human is simply mid-thought, and a sender may reasonably want
   to retry it rather than treat it as a fault.
