# Change: Tmux wedge detection and prime timeout

## Why

Today, `TmuxTransport::wait_for_quiescent_pane` runs unbounded: it waits for pane
quiescence forever, bounded only by relay shutdown. Two target failure modes are
not distinguishable from healthy behavior:

- **Unresponsive target** — during the quiescence wait for a flush group, the
  pane produces no output at all within any window and no operator-interaction
  signal is active. The transport waits indefinitely.
- **Wedged target** — output settles at a non-prompt state with no pending
  operator interaction. The transport continues to wait indefinitely.

Operators cannot distinguish "agent is thinking for a long time" from "the pane
is hung on a tool-approval dialog the prompt regex does not match" or "the
target process died without closing." A bounded prime window and a wedge
detector resolve both failure modes with distinct outcomes.

## What Changes

- Introduce a three-state delivery state machine for the Tmux transport:
  `unresponsive` (no output within prime window), `running` (output flowing or
  settled at prompt), `wedged` (output settled, not prompt-ready, no pending
  choices).
- Add a config-surfaced, opt-in prime timeout knob (`tmux-prime-timeout-ms`)
  for Tmux delivery. When the prime window elapses during the quiescence
  wait with no observable output and no operator interaction, the flush group
  resolves as `SendOutcome::Timeout` (existing variant).
- Add a config-surfaced, opt-in wedge detection knob (`tmux-wedge-detection`)
  for Tmux delivery. When the pane becomes quiescent and stays
  not-prompt-ready with no operator interaction, the flush group resolves as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`.
- Active operator-interaction signals (copy-mode or key-table for tmux)
  indefinitely suppress both `unresponsive` and `wedged` classification.
  Prime timeout does not fire while operator interaction is active.
- Modify the existing `Quiescence-Gated Delivery` requirement to clarify
  that on the Tmux transport the `quiescence_timeout_ms` field bounds the
  prime window (not the post-quiescence wait), and that the
  post-quiescence prompt-readiness wait is governed by wedge detection and
  prompt-readiness template requirements.
- Refactor `TmuxTransport::wait_for_quiescent_pane` to route the existing
  quiescence/poll machinery through the three-state classifier. The
  `DeliveryWaitError::Timeout` mapping in `wait_error_to_outcome` switches
  from `SendOutcome::Failed` + `reason_code = "quiescence_timeout"` to
  `SendOutcome::Timeout`; a new `DeliveryWaitError::Wedged` variant maps to
  `SendOutcome::Failed` + `reason_code = "pane_wedged"`.
- Introduce a `PaneQuiescenceProbe` trait in the Tmux transport module so
  tests can inject deterministic probe implementations covering the five
  behavior classes: unresponsive, wedged, pending-choice, slow-prompt, normal.

## Impact

- Affected specs: `transport-abstraction`, `session-relay`.
- Affected code:
  - `src/tmux/transport.rs` — extend `wait_for_quiescent_pane`, add
    `PaneQuiescenceProbe` trait, split `DeliveryWaitError`, update
    `wait_error_to_outcome`.
  - `src/tmux/pane.rs` — no changes expected; the probe trait is a
    transport-internal seam in `tmux/transport.rs`.
  - `src/transports/contract.rs` — add `DeliveryWaitError::Wedged` variant;
    `quiescence_timeout` field on `DeliveryEnvelope` is reused for the Tmux
    prime timeout (no schema rename).
  - `src/relay/delivery/quiescence.rs` — extend `QuiescenceOptions::for_async`
    or add a sibling constructor that accepts a prime timeout.
  - `src/configuration/**` — surface the new config keys
    (`tmux-prime-timeout-ms`, `tmux-wedge-detection`) at per-bundle and
    per-session scope; follow established hyphenated TOML key pattern from
    existing `prompt-regex` / `turn-timeout-ms` keys.
- Backwards-compatible: both new knobs default to `None`/disabled,
  preserving today's unbounded behavior. No wire-format or `SendResult` shape
  changes (Timeout variant already exists; Failed variant is reused with a new
  reason code).
- Out of scope (deferred to a follow-up OpenSpec): ACP delivery-side prime
  timeout and wedge detection implementation. The ACP wedge model is
  recorded in `design.md` as future-work context only — no active
  `session-relay` SHALL requirement for ACP is added by this proposal.