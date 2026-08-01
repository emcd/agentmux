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
  suppress injection for its iteration and continues to reset the shared
  wedge counter that Pty still depends on. Only its ability to outlive the
  readiness bound is removed.
- Leave the shared classifier machinery **intact** — the `WedgeProbe` trait,
  `QuiescenceState`, the consecutive-mismatch counter and
  `WEDGE_CONSECUTIVE_TICKS`. Pty still drives all of it. Nothing shared is
  renamed or relocated here; only Tmux's use of it goes.
- **Remove wedge detection from Tmux entirely** — the classifier, the
  `[coders.<id>.tmux].wedge-detection` knob, the counter and threshold, and the
  `pane_wedged` outcome and reason string. It infers a terminal failure from the
  absence of change in rendered content, which cannot distinguish a hung coder
  from a permission dialog awaiting an operator, a compose box holding typed
  input, or a coder working without terminal output. A send to Coordinator
  failed `pane_wedged` while its pane was blocked on a tool-permission request;
  the receipt read "settled at non-prompt state with no operator interaction"
  while operator interaction was the entire cause.
- Keep the **frame versus cursor** distinction as an expiry *reason* rather than
  a failure predicate. It retains the diagnostic value of the Band 1 work
  without the classifier that misused it.
- **Pty keeps its wedge classifier** in this change. Pty has no readiness bound
  here, so removing its only terminal path would leave it unable to end a wait
  at all — a worse regression than the one being fixed. Pty's removal is atomic
  with its bound in `agentmux:issues/relay/61`, on the same rule applied here:
  bound and removal together, never removal first.
- **BREAKING** (behavior, Tmux only): a Tmux delivery that never reaches
  readiness now resolves `Timeout` at the readiness bound instead of waiting
  indefinitely or failing `pane_wedged` seconds in. Deliveries that previously
  failed against a permission dialog, a compose box, or a briefly settled pane
  now succeed once the target returns to its prompt.
- **BREAKING** (configuration, Tmux only): `[coders.<id>.tmux].wedge-detection`
  is removed. Alpha defaults apply — the key is deleted outright rather than
  deprecated, and existing unknown-field validation covers a config that still
  sets it. An operator who set the key must therefore delete the line; there is
  no value that preserves the old behavior. The Pty key of the same name is
  unaffected.
- Delete the remaining operator-interaction suppression claims across
  `src/transports/contract.rs`, `src/tmux/quiescence_probe.rs`,
  `src/tmux/transport.rs`, and `src/configuration/raw.rs`. They document a gate
  `remove-operator-interaction-delivery-gate` already removed, and they read as
  covering the operator-composing case that Band 1 review found unprotected.
  One of them is not a comment: the Tmux wedge reason string delivered to the
  sender says the pane settled "with no operator interaction", asserting to an
  operator that their own input was ruled out as a cause when no such check
  runs.

No new `SendOutcome` variant, no relay-side backstop or outcome-policy change,
and no outcome-surface reconciliation. Every terminal result uses an existing
variant. The relay does populate the new envelope field at dispatch, alongside
the per-transport fields it already populates there.

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
  - **Three-State Delivery Classifier** — as it stands today it mandates that
    Busy suppress every terminal classification for as long as the activity
    signal is reported, and records both Tmux opt-outs as preserving unbounded
    behavior. It is rewritten so Busy suppression is bounded, so `wedged` is not
    a Tmux classification at all, and so the prime timeout is the only Tmux
    opt-out that survives.
  - **Generalized Wedge/Prime State Machine** — derives the `wait_for_change`
    deadline from `prime_timeout_ms` alone, and describes any non-empty
    non-ready tail as wedge-class. The shared machine survives for Pty; only
    Tmux's use of its `Wedged` result goes.
- `transport-contracts`:
  - **Prompt-Readiness Template Gating** — its "Do not inject while user is
    typing" scenario says the relay waits until wedge detection fires, which
    mandates wedging a composing operator. The requirement is rewritten so that
    on Tmux a readiness failure is a diagnostic distinction rather than a failure
    predicate, while Pty's frame mismatch is stated explicitly as the one
    retained exception, and gains a scenario for a pane awaiting an operator
    decision.
  - **Tmux Prime Timeout** — records an absent prime timeout as an unbounded
    wait.
- `cli-surface`:
  - **Send Timeout Override Flags by Transport** — states that the ACP and Tmux
    prime-timeout keys are the only timeout surfaces. Two keys falsify that: the
    readiness bound this change adds, and `[coders.<id>.pty].prime-timeout-ms`,
    which shipped with `add-pty-transport` and was never added here. The
    config-only invariant the requirement exists to protect is unchanged; the
    enumeration is corrected against the authoritative descriptor list in
    `addressing-routing` and marked as needing to stay current, so it does not go
    stale again the next time a key is added.
- `mcp-tool-surface`:
  - **Send Target Selection** — carries the same "only timeout surfaces"
    enumeration for the MCP payload surface, with the same correction.
- `addressing-routing`:
  - **Bundle Membership Configuration** — enumerates the descriptor fields an
    operator may write, and that enumeration is authoritative. Its
    `[coders.tmux]` list omits `readiness-timeout-ms`, and already omitted
    `prime-timeout-ms`, `wedge-detection`, ACP's `prime-timeout-ms` and Pty's
    `term-protocol`. The reconciliation adds every key the loader accepts and
    drops Tmux `wedge-detection`, which this change removes.

### Removed Capabilities

- `transport-contracts`:
  - **Tmux Wedged State Detection** — removed outright. The classification is
    unsound on a transport that can only observe rendered content, and the
    readiness bound introduced here supplies the termination it was providing.

This is the only requirement removed. The three capabilities above it have
whole-requirement `MODIFIED` deltas, not removals.

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
- Configuration, both under `[coders.<id>.tmux]`: `readiness-timeout-ms` is
  added, and `wedge-detection` is removed. The Pty key of the same name stays.
- No dependency changes and no wire-format change. There is no `format-version`
  bump and no compatibility shim. `readiness-timeout-ms` is optional with a
  default, so a config that omits it loads unchanged; a config that still sets
  `[coders.<id>.tmux].wedge-detection` does not load, because the key is deleted
  outright and existing unknown-field validation rejects it. Operators using that
  key must delete the line. That is a required edit, not a migration path.
