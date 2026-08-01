## Why

The Tmux transport can wait forever for a target that never becomes ready.
Messages into the `agentmux-aux` namespace sat in limbo for up to 68 minutes
with no diagnostic and no verdict (`agentmux:issues/relay/58`). The Band 1 fix
bounded the idle case; a target that emits output indefinitely without reaching
a prompt still never terminates, because the Busy short-circuit suppresses every
terminal classification for as long as output continues.

This is the second occurrence of one structural pattern, not a new defect.
`remove-operator-interaction-delivery-gate` deleted a gate that held a flush
group in a non-terminal state on a signal it could not bound, after it stranded
two merge-gate handoffs for 40 minutes (`agentmux:issues/relay/52`). That change
established the principle in `transport-abstraction`: a quiescence wait SHALL
always progress toward one of its terminal classifications, and the classifier
SHALL NOT hold a flush group in a non-terminal state on the basis of a rendering
signal it cannot bound. The Busy short-circuit does exactly that with a
different signal, and was added one day before the gate was removed, so the
principle was never applied to it.

This change gives that principle a bound on the Tmux transport, and generalizes
it there from rendering signals to every signal the transport cannot bound.

## Scope

**Tmux only.** This is deliberate, and it is the reason the change is landable.

The Tmux transport injects into the pane *after* its readiness wait, so an
expired bound provably precedes delivery and `Timeout` is an accurate report of
non-delivery. That is not true of the other transports: Pty writes every
envelope to the PTY master before its wait, and ACP submits the prompt before
its wait, so on those an expired bound may follow actual delivery, and reporting
non-delivery would assert what the transport cannot establish.

Bounding those correctly requires an outcome meaning "delivery unconfirmed",
which reaches eleven specs carrying outcome vocabulary, plus a per-transport
commitment-point contract and a relay-side backstop. That work is tracked as
`agentmux:issues/relay/61` and is not attempted here.

Consequently this change states a **Tmux contract**, not a universal invariant.
Pty and ACP remain unbounded after it, which is unchanged from today and is
recorded in `agentmux:issues/relay/61` rather than left implicit. The shared
state machine gains the mechanism, but Pty passes no readiness bound; the
requirement text says Tmux, and an implementation that activated the bound for
Pty merely because the code path is shared would violate it.

Two kinds of edit are therefore mixed here, and the distinction matters when
reading the deltas. The **readiness bound is new behavior, Tmux only**. The
**frame-versus-cursor split is drift repair, not new behavior**: the
`regex_matched` gate shipped on master with the Band 1 fix and applies to every
transport through the shared classifier, so requirements that describe a cursor
mismatch as eventually wedging are already wrong today. Repairing them touches
Pty-facing requirement text without changing Pty runtime behavior at all.

## What Changes

- Add a **readiness bound** for Tmux: an unconditional bound on the entire wait
  for a flush group, anchored at group formation, which no signal may defer.
  When it elapses the group resolves terminally regardless of what the target is
  doing. This is the whole of the Band 2 fix.
- Leave the Busy pre-classification's semantics **unchanged**. It continues to
  suppress terminal classification for its iteration and continues to reset the
  wedge counter. Only its ability to outlive the readiness bound is removed.
  Wedge-counter continuity therefore keeps the reset-on-activity behavior that
  makes it correct.
- Leave `WEDGE_CONSECUTIVE_TICKS` and the wedge fast path **unchanged** in
  behavior. Band 1 already guarantees observations occur on a timer, so the
  counter advances without needing re-denomination. The live specs nonetheless
  disagree about when a wedge fires — `transport-contracts` says one quiet
  window, `transport-abstraction` and `delivery-quiescence` say a
  consecutive-tick threshold, and the code implements the threshold — so the
  text is reconciled to the shipped behavior.
- Split readiness mismatch into **frame mismatch** and **cursor mismatch** in
  spec text. A healthy prompt frame with the cursor away from its idle column is
  pending operator input and SHALL NOT wedge at any tick count. This is already
  the shipped behavior after the Band 1 review; the specs still describe the two
  as one condition, and one scenario currently mandates wedging a composing
  operator outright.
- **BREAKING** (behavior, Tmux only): a Tmux delivery that never reaches
  readiness now fails within a bounded window instead of waiting indefinitely.
  The `wedge-detection = false` and absent-`prime-timeout-ms` opt-outs are
  re-scoped from "wait forever" to "no early verdict"; the readiness bound still
  applies. Alpha defaults, so no compatibility shim.
- Delete the remaining operator-interaction suppression claims across
  `src/transports/contract.rs`, `src/tmux/quiescence_probe.rs`,
  `src/tmux/transport.rs`, and `src/configuration/raw.rs`. They document a gate
  `remove-operator-interaction-delivery-gate` already removed, and they read as
  covering the operator-composing case that Band 1 review found unprotected.
  One of them is not a comment: the Tmux wedge reason string delivered to the
  sender says the pane settled "with no operator interaction", asserting to an
  operator that their own input was ruled out as a cause when no such check
  runs.

No new `SendOutcome` variant, no relay-side change, and no outcome-surface
reconciliation. Every terminal result uses an existing variant.

## Capabilities

### New Capabilities

None. Every change constrains behavior under an existing requirement.

### Modified Capabilities

- `delivery-quiescence`:
  - **Quiescence-Gated Delivery** — gains the Tmux readiness bound and the
    `readiness_timeout_ms` envelope field.
  - **Async Queue Growth Risk Disclosure** — states that the queue may grow
    without bound if targets never become ready, and directs operators to a
    `quiescence_timeout_ms` setting that does not exist.
- `transport-abstraction`:
  - **Three-State Delivery Classifier** — mandates that Busy suppress every
    terminal classification for as long as the activity signal is reported, and
    records both Tmux opt-outs as preserving unbounded behavior.
  - **Generalized Wedge/Prime State Machine** — derives the `wait_for_change`
    deadline from `prime_timeout_ms` alone, and describes any non-empty
    non-ready tail as wedge-class, which the cursor split contradicts.
- `transport-contracts`:
  - **Prompt-Readiness Template Gating** — its "Do not inject while user is
    typing" scenario says the relay waits until wedge detection fires, which
    mandates wedging a composing operator.
  - **Tmux Prime Timeout** — records an absent prime timeout as an unbounded
    wait.
  - **Tmux Wedged State Detection** — narrows to frame-absence and re-scopes the
    opt-out to the early verdict rather than the bound.
- `cli-surface`:
  - **Send Timeout Override Flags by Transport** — states that the two
    prime-timeout keys are the only timeout surfaces, which a third key
    falsifies. The config-only invariant the requirement exists to protect is
    unchanged; the enumeration is corrected and marked as needing to stay
    current, so it does not go stale again the next time a key is added.
- `mcp-tool-surface`:
  - **Send Target Selection** — carries the same "only timeout surfaces"
    enumeration for the MCP payload surface, with the same correction.
- `addressing-routing`:
  - **Bundle Membership Configuration** — enumerates the descriptor fields an
    operator may write, and that enumeration is authoritative. Its
    `[coders.tmux]` list omits `readiness-timeout-ms`, and already omits
    `prime-timeout-ms` and `wedge-detection`, both accepted by the loader since
    `tmux-wedge-detection`. All three are added.

## Impact

- `src/transports/quiescence.rs` — the readiness-bound check and outcome
  selection; removal of `unbounded_deadline()` on the Tmux path.
- `src/transports/contract.rs` — `readiness_timeout_ms` on `DeliveryEnvelope`;
  removal of the stale operator-interaction prose.
- `src/relay/delivery/dispatch/envelope.rs` — populates the field at envelope
  construction, `Some` for Tmux targets and `None` for Pty, ACP, UI, and pubsub,
  alongside the existing per-transport `prime_timeout_ms` population.
- `src/tmux/quiescence_probe.rs` — consumes the bound.
- `src/pty/delivery.rs` — passes no readiness bound; unchanged in behavior.
- Configuration: one new key under `[coders.<id>.tmux]`.
- No dependency changes. No wire-format change, and no `format-version` bump or
  migration — the new key is optional with a default, so existing `coders.toml`
  files load unchanged.
