## 1. Configuration and Envelope Surface

- [x] 1.1 Add `readiness-timeout-ms` to the per-coder `[coders.<id>.tmux]`
  config type, defaulting to `900_000` and validated to the range
  `30_000..=3_600_000`
- [x] 1.2 Add `readiness_timeout_ms: Option<u64>` to `DeliveryEnvelope` in
  `src/transports/contract.rs`, documented as bounding the entire wait for a
  flush group and as populated only for transports whose readiness contract
  defines one
- [x] 1.3 Delete the operator-interaction suppression claims from
  `src/transports/contract.rs` — both the `prime_timeout_ms` doc comment and the
  second site further down — stating that the gate was removed rather than
  silently dropping the sentences
- [x] 1.4 Populate the field in `src/relay/delivery/dispatch/envelope.rs`
  alongside the existing per-transport `prime_timeout_ms` population — `Some`
  for Tmux, `None` for Pty, ACP, UI, and pubsub

## 2. Readiness Bound in the Shared State Machine

- [x] 2.1 Thread an optional readiness deadline through
  `quiescence_classify_step`, anchored at `prime_started_at`
- [x] 2.2 Evaluate every available outcome before selecting one — delivery
  readiness and each elapsed bound — then apply precedence;
  do not add the readiness check as a positional early return, which would
  pre-empt a higher-precedence outcome
- [x] 2.3 Add the reason classifier mapping the most recent observation to a
  `Timeout` reason code, with reason precedence activity, empty tail, cursor
  mismatch, frame absence. The reason is diagnostic only — every arm is the same
  outcome — and no Tmux path produces `pane_wedged`
- [x] 2.4 Apply outcome precedence when more than one outcome is available in
  the same iteration: delivery, then the prime timeout, then the readiness
  bound. The Busy pre-classification is applied
  before outcome selection, so a prompt-ready observation whose activity
  advanced across the pair does not count as delivery-available and resolves
  `target_never_settled` when the bound has elapsed
- [x] 2.5 Cap every `NeedsWait` deadline the classifier returns at the readiness
  bound when one applies, including the Busy short-circuit's, and pass the
  earliest applicable bound to `wait_for_change`
- [x] 2.6 Ensure no iteration returns `NeedsWait` once the bound has elapsed
- [x] 2.7 Remove the unbounded fall-through for flush groups that carry a bound,
  leaving behavior unchanged for groups that do not
- [x] 2.8 Leave the Busy short-circuit's wedge-counter reset and suppression
  semantics otherwise unchanged, and leave `WEDGE_CONSECUTIVE_TICKS` unchanged

## 3. Tmux Adoption Without Pty Regression

- [x] 3.1 Consume the bound in the Tmux probe path
  (`src/tmux/quiescence_probe.rs`)
- [x] 3.2 Confirm `src/pty/delivery.rs` passes no readiness bound and its
  delivery behavior is byte-for-byte unchanged
- [x] 3.3 In the **Tmux** outcome mapping, use the frame-versus-cursor
  distinction only to select the `Timeout` reason code, never as a failure
  predicate. Leave the shared `regex_matched` wedge-class gate itself in place —
  Pty still classifies on it, and demoting it in the shared classifier would
  change Pty behavior this change has not specified

## 3b. Remove Tmux Wedge Detection

- [x] 3b.1 Stop the Tmux transport passing `wedge_detection` into the shared
  classifier, and stop it consuming `DeliveryWaitError::Wedged`
- [x] 3b.2 Delete the `wedge-detection` key from the Tmux per-coder config type,
  leaving the Pty key in place
- [x] 3b.3 Remove the `pane_wedged` reason string and the
  `delivery_pane_wedged` diagnostic from the Tmux path, including the
  operator-facing wedge reason text that claims operator interaction was ruled
  out
- [x] 3b.4 Leave the shared `WedgeProbe` trait, `QuiescenceState`, the
  consecutive-mismatch counter and `WEDGE_CONSECUTIVE_TICKS` intact — Pty still
  drives them. Do not rename or relocate shared machinery in this change
- [x] 3b.5 Remove Tmux wedge tests, and confirm the Pty wedge tests still pass
  unchanged as the evidence that removal was Tmux-scoped
- [x] 3b.6 Confirm the terminal-outcome receipt path carries the readiness
  expiry to the sender with its reason code, and that no receipt test still
  asserts a Tmux `pane_wedged` outcome

## 4. Coverage

- [x] 4.1 Unit coverage that a Tmux target whose activity advances on every
  observation terminates on the readiness bound with `target_never_settled`
- [x] 4.2 Unit coverage for each reason-classifier arm at expiry: empty tail,
  cursor mismatch, and frame absent — each asserting `Timeout` with its reason,
  and none producing `pane_wedged`
- [x] 4.3 Unit coverage for reason precedence when more than one condition holds
  in the same observation pair
- [x] 4.4 Unit coverage that a prime timeout elapsing alongside the readiness
  bound resolves as the prime timeout (the delivery tier is covered by 4.9)
- [x] 4.5 Unit coverage that a settled Tmux pane with the frame absent produces
  no terminal outcome before the bound, whatever the pane content
- [x] 4.6 Unit coverage that an absent prime timeout still terminates on the
  readiness bound
- [x] 4.7 Unit coverage that no returned `NeedsWait` deadline exceeds the bound
- [x] 4.8 Unit coverage that a flush group carrying no readiness bound is
  classified exactly as it was before this change, so the shared code path does
  not impose the bound on Pty
- [x] 4.9 Unit coverage for the delivery/expiry race: a prompt-ready observation
  in the iteration the bound elapses resolves `Delivered` and injects, while the
  same observation with the activity signal advancing resolves `Timeout` with
  `target_never_settled` and injects nothing
- [x] 4.10 Configuration coverage for `readiness-timeout-ms`: absent takes the
  `900_000` default, `30_000` and `3_600_000` are accepted, and values below or
  above the range are rejected with a structured error naming the key
- [x] 4.11 Envelope-population coverage: a Tmux target's envelope carries
  `Some(bound)` and Pty, ACP, UI, and pubsub targets each carry `None`.
  Implemented by extracting the per-transport rule to
  `TargetConfiguration::readiness_timeout_ms`, which the envelope builder is now
  the sole caller of, and covering all five variants there — the envelope
  builder itself is `pub(super)` and unreachable from an integration test
- [x] 4.12 Coverage that the readiness bound is taken from the head envelope and
  does not shift when later envelopes are absorbed by coalesce-during-wait,
  matching the existing prime-window anchoring rule
- [x] 4.13 Teeth-check each new test by reverting its corresponding source change
  and confirming failure. Every assertion in this section describes behavior that
  does not exist yet — a wait that terminates, a key that validates, a field that
  is populated, a bound that survives coalescing — so a test that passes against
  unmodified source is asserting nothing

## 5. Documentation

- [x] 5.1 Update the quiescence subsystem README where it describes the Tmux
  wait as bounded by wedge detection, and record that Tmux no longer classifies
  `wedged` while Pty still does
- [x] 5.2 Update operator-facing async-delivery documentation to describe the
  Tmux readiness bound, name the transports that remain unbounded, and drop the
  `quiescence_timeout_ms` reference. The operator surface is
  `documentation/usage/maintainer-configuration-guide.md`, which had no
  async-delivery bounds section at all — the `quiescence_timeout_ms` reference
  the task anticipated lives only in the spec text this change already
  rewrites, so there was none to drop here
- [x] 5.3 Record in the `quiescence_classify_step` doc comment that the bound is
  Tmux's unconditional termination guarantee rather than its only terminal path —
  an opted-in prime timeout, shutdown, and a positively observed probe failure
  remain — that absence of activity is not evidence of failure, and that
  `pane_dead` is the only sound Tmux failure signal and is unreachable without
  `remain-on-exit`
- [x] 5.4 Triage every surviving reference to the removed operator-interaction
  gate and delete the stale ones. Enumerate with
  `grep -rnE "operator.interaction" src/ documentation/architecture/openspec/specs/`,
  which finds **16 sites across 8 files** on master `26c18a0` — measured after
  `issues/relay/60` removed five of them, and still more than the handful
  visible from reading the quiescence path alone. Not all 16 are stale: some
  accurately state that Pty never had the concept. Each site needs a decision,
  not a blanket deletion, and the count is recorded so an implementer can tell
  whether the surface has moved since. Exclude `changes/archive/`, whose
  proposals legitimately describe what they removed. This overlaps
  `todos/general/39`, which proposes making the sweep mechanical; doing the
  triage here supplies that item's seed data either way. The operator-facing
  wedge reason string in `src/tmux/transport.rs` is not part of this triage — it
  is deleted outright by task 3b.3, not rewritten.

  **Triage result.** The recorded command under-reports: `grep` is line-based,
  and rustfmt wraps the phrase across lines in doc comments. It finds 8 sites;
  `rg --multiline` finds **12 sites across 4 files**. That gap is the strongest
  argument for `todos/general/39` making the sweep mechanical, and the seed data
  for it is a multiline matcher, not the recorded one.

  Deleted as stale (4): the two ACP comparison anchors
  (`src/acp/README.md`, `src/configuration/types.rs` on
  `AcpTargetConfiguration::prime_timeout_ms`), which explained ACP's lack of
  wedge detection by comparison to a Tmux classifier that no longer exists;
  the Tmux `prime_timeout_ms` doc claiming the timeout fires only when no
  operator interaction is active; and the `DeliveryWaitError::Wedged` doc
  carrying the same claim.

  Kept as accurate (8): the two spec re-scoping notes recording that
  `operator_interaction_active` was retired (`transport-abstraction` 1071-1081,
  `transport-contracts` 64-68) — provenance, not live behavior — and the
  `transport-contracts` Pty prime-timer statements at 1084/1086, which
  correctly say Pty never had the concept. No live spec was edited, which is
  also the right outcome procedurally: live specs are updated at sync, not
  during apply
