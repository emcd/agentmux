# src/tmux/

Tmux transport: pane operations, session lifecycle primitives, and the
[`Transport`](crate::transports::Transport) implementation.

The relay delivery worker dispatches tmux delivery generically through
[`TmuxTransport`](transport::TmuxTransport); the relay orchestration
layer (bundle reconcile/startup/shutdown) calls the lifecycle primitives
in [`lifecycle`] directly.

Dependency direction is downward only: this module depends on
`crate::transports`, `crate::configuration`, and `crate::runtime`, never
on `crate::relay`. The lifecycle primitives surface a transport-local
[`TmuxLifecycleError`](lifecycle::TmuxLifecycleError); relay maps it to
its own `RelayError` envelope at the orchestration boundary via a
`From` impl that lives in relay, so no tmux->relay back-edge is
introduced.

## Module layout

- `lifecycle` — session lifecycle primitives (create / kill / bind),
  driven by relay bundle reconcile/startup.
- `pane` — pane operations: resolve active pane target, inject literal
  text into the pane, capture pane tail lines.
- `prompt_probe` — the pane-readiness probe the transport's observer thread
  refreshes and its delivery-loop executor reads before each write. Implements
  [`PanePromptProbe`].

  OpenCode readiness has a second, private compose-region gate. It recognizes
  the measured 1.18.9 frame suffix (info row, 20-or-more `▀` characters, and
  the `ctrl+p commands` status row), then checks exactly the three input rows
  before the info row. Two captured 1.18.9 layouts establish the 99/100-space
  boundary: 99 spaces can be compose text, while 100 or more belongs to the
  sidebar. A future OpenCode layout change requires revisiting these bounds.

   Tmux does **not** classify `wedged`. Inferring a terminal failure from
   the absence of change in rendered content cannot distinguish a hung
   coder from a permission dialog awaiting an operator, a compose box
   holding typed input, or a coder working without terminal output. The
   executor therefore reads the cached pane observation as an advisory
   readiness level and, when it is ready, declares and pastes immediately.
   It performs no readiness wait after declaring.
- `transport` — [`TmuxTransport`] (the per-target `Transport`
  implementation, its observer thread, and its one serial delivery-loop
  executor) plus [`render_paste_text`] (the per-envelope pane-text
  rendering used by `paste_group`).
- `transport::executor` — `TmuxDeliveryWriter`, this transport's half of the
  shared delivery loop: what it peeks, how it packs a peeked run into one
  paste, and what a paste proves.

## Terminal-outcome receipt rendering

Terminal-outcome receipts (relay/system-originated envelopes carrying
the outcome of a prior async send, routed to the original sender
through the sender's own Tmux transport) are rendered with a leading
marker line so the receiving agent can distinguish them from peer
messages at a glance. The marker line is emitted only for receipt
envelopes; peer envelopes render unchanged. Detection uses the
`DeliveryEnvelope.is_receipt` field the relay propagates from the
receipt builder — see `build_coder_envelope` in
`src/relay/delivery/dispatch/envelope.rs`. Receipts are non-recursive
at the relay's terminal-resolution chokepoint; the Tmux transport does
not enforce or check that invariant.

The marker is included in the rendered text so the token-budget
batching and paste-budget counts stay consistent with the actual
pane bytes. The marker line and the rendered pane envelope are
contiguous in the pasted prompt.

**A receipt is never pasted alongside peer traffic.** `coalescing_runs`
splits a peeked run at receipt boundaries *before* token budgeting, so
every receipt is injected alone and peer envelopes coalesce only within
peer-only runs. That separation is presentation rather than a custody
rule: a receipt is declared and acknowledged exactly like its peer
neighbours — what it lacks is a quota reservation, not a mailbox position
— so a prompt carrying both would not tie them to different fates. What it
would do is render as one message to the agent reading it, with the marker
line that identifies a receipt stranded in the middle of somebody else's
text.

ACP reaches the same rule from the other direction (receipts are its
flush barrier), and Pty writes one entry per primitive, so neither needs
this. See `src/relay/delivery-architecture.md`, "Members no reservation
covers".

The `render_paste_text` helper is the per-envelope rendering seam
and is `pub` so the rendering behavior can be tested directly
(`tmux_transport_render_paste_text_emits_receipt_marker_for_receipt_only`
in `tests/unit/tmux_transport.rs`). `paste_group` consumes it as
part of its budget-batched prompt assembly; the marker is included
in the prompt before token budgeting and paste injection.

## Cross-references

- `src/transports/contract.rs` — the transport contract and the
  `DeliveryEnvelope` / `DeliveryMessage` types the relay populates
  and the Tmux transport renders.
- `src/transports/diagnostics.rs` — the transport-neutral delivery-progress
  inscription context used by transports to bound diagnostic message IDs and
  emit progress inscriptions. It does not classify readiness or delivery
  outcomes.
- `src/envelope.rs` — the canonical pane-envelope renderer
  (`render_envelope`) and `AddressIdentity` helpers. The Tmux
  transport renders one pane envelope per `DeliveryEnvelope.message`
  via `DeliveryMessage::render_pane_envelope`, which is the
  transport-neutral renderer; the marker line is prepended by the
  Tmux-specific `render_paste_text` before the budget batching
  and paste injection.
- `src/pty/README.md` — the Pty transport's receipt rendering
  writeup, which uses the same marker literal for cross-transport
  consistency.
- `src/relay/delivery/async_worker/` — the relay-side terminal-
  resolution chokepoint (`terminal.rs`, `complete_task_outcome`) and the
  receipt construction it hands off to (`reporting.rs`,
  `deliver_terminal_outcome_receipt` → `build_terminal_outcome_receipt`),
  including the non-recursion enforcement at the single spawn site.
- `documentation/development/README.md` — build prerequisites, lint /
  test gates, and the tmux test harness setup.
