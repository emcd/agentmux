## Context

`SendOutcome` is the shared per-target delivery vocabulary in
`src/transports/vocabulary.rs`, consumed by every transport, by the relay's
async delivery worker, and by the TUI. The `Timeout` variant dates from an era
when transports resolved a flush group at a prime, readiness, or quiescence
bound. All three bounds have since been deleted, and the execution watchdog that
replaced them resolves still-unresolved members through the fence verdict's
evidence order — `not_submitted` when non-delivery is provable,
`submission_unknown` when it is not. Neither spelling is `Timeout`, by design:
the watchdog measures relay-side execution overrun, not target health.

This change is cross-lane because the variant is sender- and TUI-visible. It is
not, however, cross-lane in its **spec** surface, which is the main finding of
the survey below.

## Goals / Non-Goals

**Goals:**

- Remove `SendOutcome::Timeout` and the `timeout` outcome spelling from the
  delivery vocabulary, including the relay's stream and receipt renderings and
  the TUI's consumption of both.
- Leave the TUI's stream-side terminal vocabulary consistent with what the relay
  actually emits.

**Non-Goals:**

- Reconciling `add-opencode-compose-readiness-contract`. Its
  `transport-contracts` delta cites `SendOutcome::Timeout` and is separately
  stale against deleted per-coder `prime-timeout-ms` keys. That belongs to
  whoever owns that change.
- Re-litigating what replaced the timeout bounds. The evidence order is settled
  contract; this change only removes the vocabulary entry it orphaned.
- Any rejection path, error code, or test asserting that `timeout` is now
  refused. Per the project's alpha posture, a dropped variant is dropped
  outright; existing unknown-field handling already covers the wire side.

## Decisions

### Three requirements need deltas, and the first survey found only one

The dispatch anticipated normative deltas across `delivery-quiescence`,
`transport-contracts`, `transport-abstraction`, `cli-surface`, and
`tui-surface`. The answer is neither that list nor the single delta the first
pass concluded: it is `tui-surface`, `look-and-stream-events`, and exactly one
requirement inside `transport-contracts`.

**How the first survey went wrong, since the method matters more than the
result.** It searched for the qualified `SendOutcome::Timeout` and mapped each
hit to its enclosing requirement. That is sound for finding where the *Rust
variant* is named, and it is useless for finding where the *wire spelling* is
normative — and the wire spelling is what a sender- and TUI-visible deletion
actually breaks. Two requirements specify `outcome=timeout` without ever naming
the Rust type, so no qualified search could have surfaced them:

- **`look-and-stream-events` → Relay Stream Event Contract** permits
  `outcome` (`success`|`timeout`|`failed`|null) and defines the failure
  terminal as `outcome` in (`timeout`|`failed`). No in-flight change touches
  this capability at all. Deleting the sole producer of that spelling would
  leave a live wire contract for an outcome nothing can emit.
- **`transport-contracts` → ACP Stop-Reason Outcome Mapping** maps an ACP turn
  timeout to delivery outcome `timeout` with `reason_code = acp_turn_timeout`.
  It is the one `transport-contracts` requirement that
  `establish-delivery-commit-contract`'s delta does not cover — its four
  transport-contracts targets are the prompt-readiness and three prime-timeout
  requirements.

  This one is **REMOVED rather than MODIFIED**, which the first attempt got
  wrong by editing out the timeout bullet and retaining the rest. The retained
  mappings were also false. An ACP delivery resolves `Delivered` from typed
  submission evidence at the framed `session/prompt` write — every member of the
  group resolves before the replay-buffer locks or `on_dispatched` run — and the
  turn lifecycle that follows is observability only. A completion arriving later
  therefore cannot select a delivery outcome: the stop reasons said to map to
  `delivered` assert a causality that no longer exists, since the outcome would
  be `delivered` whichever reason arrived and even if none did. `cancelled` ->
  `failed` with `acp_stop_cancelled` has no implementation anywhere in the
  source. Dropped-on-shutdown is a shutdown-lifecycle behavior housed here by
  category error. Editing one bullet out of a requirement whose remaining
  content is equally false would have laundered three wrong claims through a
  change that touched the fourth.

The corrected method is to sweep for the **bare** term in an outcome context and
triage each hit, then apply the enclosing-requirement mapping below. Task 4.1
enforces this at the end rather than trusting the survey.

With that correction, the enclosing-requirement mapping still holds for
everything it did cover:

- **`transport-contracts`** — the citations fall inside *Prompt-Readiness
  Template Gating*, *ACP Prime Timeout*, *Tmux Prime Timeout*, and *Pty Prime
  Timeout*. `establish-delivery-commit-contract` already carries the first as a
  MODIFIED requirement and the other three as REMOVED requirements.
- **`transport-abstraction`** — the citations fall inside *Three-State Delivery
  Classifier* and *ACP Prime Timeout Envelope Field Consumption*, both REMOVED
  by that same change.
- **`delivery-quiescence`** — the citations fall inside *Quiescence-Gated
  Delivery* and *Asynchronous Terminal-Outcome Receipt*, both MODIFIED by it.

A MODIFIED delta replaces the whole requirement, and none of that change's
deltas reintroduce `SendOutcome::Timeout`. So every citation in those three
specs disappears when that change archives, whether or not this one exists.

That last claim needed one check the first survey missed, since a MODIFIED
delta replaces a requirement with its own text: a `Timeout` surviving in the
**delta text** would land in the live spec on archive rather than vanish from
it. Five bare `Timeout` strings do remain in that change's deltas. Four are
rationale prose inside REMOVED requirement sections, which sync never copies
into a live spec, and the fifth sits in an ADDED requirement where it is the
ordinary English word for an elapsed observation window in the fencing verdict,
not the variant. None of them lands.
Writing deltas against the same requirements would duplicate a removal already
in motion and put two changes in conflict over identical requirement blocks —
the precise hazard that deferred this deletion in the first place.

**`cli-surface`** was checked and has no dependency at all: its timeout
requirements govern per-coder timeout *configuration keys* and retired CLI
*flags*, which are a different subject from the outcome vocabulary.

Alternative considered: write deltas everywhere for completeness. Rejected — it
manufactures cross-lane conflict to restate removals that already exist, and it
would leave this change unarchivable until the other one lands.

### The wire spelling dies with the variant, in one place

`emit_sender_delivery_outcome_event` is the sole mapping from a terminal
`SendOutcome` to the `outcome` field of a `delivery_outcome` stream event; the
dispatch envelope's phase emitter is a pass-through of that mapping rather than
an independent producer. Its `Timeout` arm is what produces `outcome:
"timeout"`, and it is the only thing that does. Deleting the variant therefore
makes the TUI's two `"timeout"` string arms dead by construction rather than by
convention, which is what lets them be deleted in the same change as the enum.

The `relay.send.async.completed` inscription serializes the outcome through
serde, so `timeout` leaves the log vocabulary at the same time. No separate
work: the variant's removal is the whole mechanism.

### Deserialization failure is accepted, not guarded

`SendOutcome` is a deserialized field on the relay and transport contract
payloads, and it carries no `serde(other)` fallback. After this change, an
inbound payload carrying `"outcome": "timeout"` fails the **whole payload**
rather than mapping to a variant or silently remapping to something
adjacent-but-wrong. The only realistic source is a peer relay at an older
revision on a cross-relay bang-path target.

Accepted, and deliberately not mitigated. Adding a tolerant alias would keep the
spelling alive in exactly the place the change is trying to remove it from, and
the project's alpha posture prefers a fail-fast to a compatibility shim. This is
recorded because it is the one consequence that leaves the process, not because
it needs handling.

### Correcting the TUI stream vocabulary is in scope

Argued in the proposal, and approved. In short: the requirement being modified
enumerates the TUI delivery vocabulary, and the same match arms being edited to
drop `timeout` are the ones silently discarding `not_submitted` and
`submission_unknown`. Restating the vocabulary while omitting two spellings the
relay already emits would land a knowingly false spec.

**What the defect actually is** — the first statement of it was wrong in a way
that would have produced a test with no teeth. Clearing a pending delivery does
not depend on the outcome spelling: any non-`routed` phase removes the id, so an
already-pending send resolves regardless. The outcome spelling gates entry into
the *terminal-message* set, and that set exists to guard the case where a
terminal event arrives **before** the queued acknowledgement. Because the two
evidence-bearing outcomes never enter it, a terminal event that wins that race
leaves the later acknowledgement free to insert the message as pending, and the
event that would have cleared it has already passed.

The consequence for testing is the reason this is worth stating precisely: an
event-after-pending test passes against the unfixed code. Only an
event-before-acknowledgement ordering discriminates, which is what task 3.4
requires.

### `peer_unavailable` is left out of the stream contract deliberately

The relay's terminal-outcome mapping has a `PeerUnavailable` arm that would emit
`phase=failed`, `outcome=peer_unavailable`, and the stream contract enumerates
no such outcome. The arm's own comment states it is unreachable on this path —
a cross-relay peer-unavailable result is reported synchronously on the send
response, and the arm is defensive. Adding it to the normative enumeration would
assert a stream behavior the code says cannot occur, and removing the defensive
arm is outside this change. Recorded as a known gap rather than resolved either
way.

## Risks / Trade-offs

- **The spec survey is the load-bearing claim, and its first pass was wrong.**
  A qualified-name search cannot find a requirement that specifies the wire
  spelling without naming the Rust type, which is how both
  `look-and-stream-events` and the ACP stop-reason requirement were missed. →
  Corrected by sweeping the bare term and triaging each hit; task 4.1 repeats
  that sweep at the end so a further miss surfaces as a failing check rather
  than as drift. The residual risk is a normative constraint that spells the
  concept without using the word `timeout` at all, which no textual sweep
  catches — reviewers with delivery-path context are the control for that, not
  the search.

- **This change must not archive before `establish-delivery-commit-contract`.**
  Archiving first publishes a live spec set whose `delivery-quiescence`,
  `transport-contracts`, and `transport-abstraction` requirements name a variant
  the code no longer defines. → Sequencing constraint recorded in tasks, with
  task 4.1 as the enforcement point. Its fourth permitted exception covers those
  not-yet-archived requirements so the sweep stays runnable at any time, but it
  is a staging allowance rather than an archive allowance: run 4.1 after the
  delivery-contract change archives, and a non-empty fourth category fails
  archive readiness for this change.

  An intermediate revision relaxed this constraint on the grounds that the delta
  targets are disjoint. Disjointness rules out a merge conflict and says nothing
  about live-spec coherence, and the supporting argument — those requirements
  are already stale, so one more wrong detail is harmless — conflated drift a
  reader can discover with a mechanically false type reference. Restored.

- **Correcting the TUI vocabulary changes rendered output** for outcomes that
  currently display as an unknown placeholder. → That is the point, and the
  behavior it replaces is a defect rather than a contract anyone depends on.

- **The exhaustive-vocabulary assertion in the relay's evidence-authority tests
  is deliberately exhaustive** so a vocabulary change cannot pass unnoticed.
  Removing a variant will fail that match until it is updated. → Expected, and
  the reason that site is listed as implementation work rather than incidental
  cleanup. It is the mechanism working.

### The outcome-label list loses `timeout` too

Settled with the backend lane rather than left open, because the first reading
of it was wrong in an instructive way.

`labels_an_outcome` is a **strip filter**, not a rejection: `reconcile_with_evidence`
discards a reason code that merely restates the outcome, because `delivered`
sitting on a `submission_unknown` contradicts the field beside it. Causal codes
survive — that is what keeps `pty_write_failed` attached to the outcome it
explains.

So retaining `"timeout"` after the variant is gone would not prevent a
transport from emitting a confusable label; it would silently **destroy** a
causal diagnosis, since there would no longer be an outcome named `timeout` for
it to contradict. The list's own membership rule settles it: every terminal
`SendOutcome` wire label belongs there, and `timeout` stops being one.

The exhaustive assertion is the decisive argument, read the right way round. It
derives the expected classification from the `SendOutcome` variants themselves,
so deleting the variant removes both the array entry and the match arm. A
`"timeout"` left in the classifier would then be the single hand-maintained
entry in a list whose entire purpose is that it is not hand-maintained — the
exact debt that assertion was introduced to retire.

## Open Questions

None outstanding.
