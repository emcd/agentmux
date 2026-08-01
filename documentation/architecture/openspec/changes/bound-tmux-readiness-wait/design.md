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
- Preserve the diagnostic detail the wedge path provided, as timeout reasons
  rather than as a failure predicate.

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

### Remove the Tmux wedge classifier rather than narrow it again

Three successive changes tried to make the classifier correct by refining what
counts as wedge-class: `322bf80`, then `3fe6fb8`, then the frame-versus-cursor
split in this proposal's earlier drafts. Each traded one false positive for
another. The pattern is not bad luck; the classifier asks a question its inputs
cannot answer.

A settled non-prompt frame is produced by at least four conditions: a hung
coder, a permission dialog awaiting an operator, a compose box holding typed
input, and a coder working with no terminal output. From `capture-pane` these
are identical. Any predicate over that content classifies all four the same way,
so a predicate that catches the hang necessarily misreports the other three.

The cost is asymmetric, which is what settles it. Every false positive is a
message that failed and should have landed — unrecoverable, and the sender was
told something untrue. The only cost of not classifying is latency before a
genuinely hung pane is reported, which the readiness bound now supplies. And the
benign cases are the common ones while a genuinely hung coder is rare, so the
classifier bought speed on a rare condition by being wrong on frequent ones.

The incident that forced this: a send to Coordinator resolved `pane_wedged`
while its pane was blocked on a tool-permission request. Not hung — waiting on a
human. The receipt read "settled at non-prompt state with no operator
interaction" while operator interaction was the entire cause.

The boundary this draws is **positive evidence versus inference**, not transport
identity. Activity advancing is a one-way positive signal and continues to
suppress injection; its absence is evidence of nothing. Only a positively
observed terminal event is sound: process death, a closed connection, a protocol
error. Tmux does expose `pane_dead`, but only under `remain-on-exit`, which this
system does not set — without it a dead process destroys the pane, and the
resulting probe failure already resolves the wait. `pane_pid` liveness proves
nothing, since hung processes are alive. `pane_current_command` looks like
positive evidence and is not: a coder that exited to a shell and a coder running
a shell tool call both report the shell. So the sound-signal door is left open
deliberately, not overlooked, and nothing behind it is currently reachable.

Pty is excluded from the removal, and the exclusion is a **knowing violation of
the rule above, not an application of it**. Pty's classifier draws exactly the
unsound inference this section rejects. It survives only because deleting it
without first supplying a bound would leave Pty with no terminal path at all — a
strictly worse regression than the one being fixed, shipped inside the fix. Its
removal is atomic with its bound in `agentmux:issues/relay/61`.

The distinction matters for how the requirements are worded. Since the boundary
is evidence quality rather than transport identity, "this transport has no
readiness bound" must not become the criterion that licenses `wedged`. Written
that way it would read as a general rule and would authorize the next
bound-less transport to infer failure from a static screen. The deltas therefore
state the classification as unsound outright, name Pty as the single retained
exception, and give the exception an expiry.

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
was available — a delivery the target had just become ready for, or a prime
timeout.

The contract is therefore stated as two steps. First, evaluate every available
outcome — delivery readiness and each elapsed bound. Second, apply precedence. What the bound
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

On Tmux, nothing derived from pane content produces a failure any more:
`pane_wedged` does not exist on this transport. Pane-content observation feeds
exactly two outcomes — `Delivered` when the target is prompt-ready, `Timeout`
when a bound elapses.

The transport's other terminal paths are unchanged and are not derived from what
the pane shows: an opted-in prime timeout resolves `Timeout`, relay shutdown
resolves `Shutdown`, and a positively observed probe or transport failure
resolves `Failed`. So "the readiness bound is the sole terminal path" would be
wrong; the accurate claim is narrower and stronger — the bound is the
*unconditional* one. Every other path is conditional on something (an operator
opting in, the relay stopping, a probe erroring), and the bound is what
guarantees that one of them arrives.

The reason attached to a timeout is diagnostic only — every arm below is the same
outcome, and the distinction exists so an operator can tell a short bound from a
stuck target, not so the transport can decide differently:

| Most recent observation at expiry | `reason_code` |
|---|---|
| Prompt frame absent | `target_not_ready` |
| Inspected tail empty | `target_unresponsive` |
| Frame present, cursor off idle column | `pending_operator_input` |
| Activity advancing | `target_never_settled` |

`target_not_ready` covers the four indistinguishable cases together — hung
coder, permission dialog, compose box, silent work — precisely because they are
indistinguishable. Naming one of them would be a guess dressed as a diagnosis.

Two orderings, easily conflated:

- **Reason precedence** within one observation pair, highest first: activity
  advancing, then empty tail, then cursor mismatch, then frame absence. Activity
  ranks first because it is the only signal describing the pair rather than the
  final snapshot.
- **Outcome precedence** when more than one outcome is available in the same
  iteration, highest first: delivery, then the prime timeout, then the readiness
  bound. On a transport that still classifies `wedged` — Pty today — that sits
  between delivery and the prime timeout.

Delivery ranking first is the non-obvious one. A target that becomes prompt-ready
in the very iteration the bound expires is delivered to, not timed out: reaching
readiness late is the outcome the wait existed to obtain, and the bound's purpose
is to stop waiting forever rather than to refuse a success already in hand. It
also matches the existing branch order, where the prompt-ready check already
precedes the prime-timeout check, so this extends a rule rather than inventing
one. Busy is unaffected — a prompt-ready observation whose activity advanced is
still deferred, and an elapsed bound then resolves it as `target_never_settled`
rather than granting the match Busy had just denied.

The prime timeout outranks the readiness bound as the more specific diagnosis,
reached only when an operator opted into it.

The three states remain mutually exclusive at the moment of terminal
classification, as the existing requirement demands.

## Risks / Trade-offs

- **A long agent turn fails a delivery that would have succeeded** → The
  substantive risk, and why the default is 15 minutes rather than something
  tighter. Mitigated by the per-coder key, by a lower range bound that prevents
  configuring below one turn, and by `target_never_settled` naming the state the
  wait ended on — which is calibration input, not a diagnosis. It says the
  target was still producing output when the bound arrived; whether that was a
  turn needing more time or a target that will never finish is exactly what the
  transport cannot determine, and the reason code does not claim to. On Tmux the
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
  → The Tmux prime-timeout opt-out currently means "wait forever"; it is
  re-scoped to "no early verdict", with the readiness bound applying regardless.
  The Tmux wedge opt-out is not re-scoped — it goes away with the classifier, and
  an operator who set `wedge-detection = false` was already getting the behavior
  this change makes unconditional. Both are called out as BREAKING rather than
  shimmed, and the removed key requires an operator edit (see Migration Plan).
- **A deleted safety claim may read as a removed feature** → The
  operator-interaction prose documents a gate already deleted, so only stale
  text goes. The risk is the inverse: a reader may conclude the composing case
  is still covered, which is how this text misled the Band 1 review.

## Migration Plan

No data migration and no persisted-state shape change. Rollback is a revert.

Configuration is not migration-free, though there is no migration *path*.
`readiness-timeout-ms` is optional with a default, so adding it needs no operator
action. Removing `[coders.<id>.tmux].wedge-detection` does need one: the key is
deleted outright rather than deprecated, so a config still carrying it fails load
on existing unknown-field validation, and the operator must delete the line. No
`format-version` bump accompanies that, because the schema version is not what
tells the operator — the load error is. The identically named Pty key stays.

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
