# Design: Async terminal-outcome receipt

## Context

Relay accepts sends asynchronously (Delivery Results Without ACK Protocol):
an accepted target returns `outcome = queued` immediately, and the relay never
blocks the caller. The delivery worker later resolves each queued flush group to
a terminal outcome — `delivered`, `SendOutcome::Failed` (with `reason_code`,
including `pane_wedged`), `SendOutcome::Timeout`, or `dropped_on_shutdown`.

Two facts are settled and shape this design:

- **Submission-time rejection already exists.** The unified registry requirement
  makes absent targets `validation_unknown_target` and present-but-incapable
  targets `validation_unsupported_operation`, while "offline is a state, not
  absence" keeps a configured-but-not-ready target routable. Decision-point 1 is
  done; this change does not touch it.
- **There is no push channel to a coder sender.** Relay stream events are a
  UI/TUI-only mechanism. `send_event_to_registered_ui` gates on
  `session_type == Ui` (`src/relay/stream/mod.rs`); a coder Hello binds its
  session type to the bundle transport, never `Ui`
  (`src/relay/connection/helpers.rs`); and the coder MCP connection is
  request/response only — it does not poll for events and explicitly discards any
  pushed frame (`src/mcp/server/service.rs`, `mcp.tool.stream.events_ignored`).
  Coder agents receive through their transport (pane injection for Tmux, a turn
  for ACP, a pty write for Pty), not through pushed frames. The existing
  sender-directed push (`emit_sender_delivery_outcome_event`) already fires but
  lands only for a UI-class sender.

So the terminal outcome must reach the sender the same way any message does:
through the sender's own transport.

## Goals

- The original sender learns of a non-delivered terminal outcome for its queued
  message, through its own transport, without reading `relay.log`.
- Reuse the existing per-transport delivery pipeline; add a message type, not a
  new channel.
- Keep `relay.log` an always-on floor for every terminal outcome.
- Keep the change clear of store-and-forward and pub/sub.

## Non-Goals

- Success receipts: a `delivered` outcome is recorded in `relay.log` only.
- Deferred delivery / mailboxes: no persistence when the sender is not routable.
- Pub/sub transport.
- Any change to submission-time rejection or to async-only submit (`queued`
  remains the accept-time response).

## Decisions

### D1 — A terminal-outcome receipt is a new envelope type routed via the sender's transport

When a queued message `M` (sender `S` → target `T`) resolves to a **non-delivered**
terminal outcome, the relay enqueues a **terminal-outcome receipt** addressed to
`S` and delivers it through `S`'s own transport via the existing delivery
pipeline. Because delivery is already transport-abstracted, "each transport its
own way" comes for free: Tmux renders the receipt into the pane, ACP as a turn,
Pty as a pty write, and the UI transport as the `delivery_outcome` stream frame
that `emit_sender_delivery_outcome_event` already emits. The receipt carries the
original `message_id`, the delivery target `T`, the terminal outcome, and any
`reason_code`, so `S` correlates it to the `queued` result it received at accept
time.

The receipt is a distinct envelope/message **type**, not a peer `incoming_message`.
It is addressed and routed from the **sender's** bundle and runtime directory
(the sender's home bundle for a cross-bundle send), not the target's — the
delivery route is the sender's, so building the receipt's delivery task from the
target's runtime directory would misroute it.

### D2 — Non-delivered outcomes only

Receipts are delivered for `failed` (including `pane_wedged`), `timeout`, and
`dropped_on_shutdown`. A `delivered` outcome produces no receipt; it is recorded
in `relay.log` only. Delivering a success receipt for every send would double
delivery traffic through the real transport and spend the sender's tokens and
attention on the common case; the silence that hurt in relay/52 was specifically
non-delivery.

### D3 — Relay/system is the receipt sender; receipts are not recursive

The receipt is relay/system-originated, not attributed to a peer principal, so an
agent can distinguish "your message to `T` wedged" from ordinary inbound peer
traffic. It carries a marker distinguishing it from a peer message — a boolean
flag on the envelope or a dedicated message variant (an implementation-phase
choice: a dedicated variant is more type-safe, a flag is less churn).

The receipt-emission decision lives in the **relay**, not the transports. The
relay's terminal-resolution point is the single chokepoint: it spawns a receipt
only when the resolving delivery is itself NOT a receipt and its outcome is
non-delivered. That single spawn-site check is the whole non-recursion
enforcement. Coder transports MAY inspect the typed `DeliveryEnvelope.is_receipt`
flag the relay populates to apply per-transport rendering polish (e.g., ACP's
flush-barrier behavior, Pty's leading marker line, Tmux's pane-text marker
inclusion before token-budget batching), but receipt emission and the
non-recursion invariant remain solely the relay's concern.

A receipt is itself a delivery. Its own terminal outcome SHALL NOT spawn another
receipt — receipts are non-receipted — or a wedged/failed receipt would loop
forever. A receipt's own terminal outcome is recorded in `relay.log` and stops
there.

### D4 — Best-effort; drop when the sender is not routable

The receipt is enqueued through the normal delivery route to `S`. It is
best-effort: if `S` is not routable — its delivery worker/session does not exist
(offline/unhosted) — the receipt is dropped, not persisted, queued indefinitely,
or retried — the deliberate boundary against deferred delivery. A
wedged-but-registered sender is routable-but-slow: the receipt queues behind its
in-flight deliveries rather than dropping. `relay.log` still records the
underlying terminal outcome regardless. "Offline is a state" applies as it does
for any delivery to `S`.

### D5 — `relay.log` is the completeness floor

Every terminal outcome — delivered and non-delivered — is recorded as an
inscription regardless of whether a receipt is sent or lands, so nothing is lost
when a sender is absent or when the outcome is `delivered`. Async Delivery
Observability is widened from its current `delivered` / `timeout` /
dropped-on-shutdown enumeration to the full terminal set: `delivered`, `failed`
(including `pane_wedged`), `timeout`, and `dropped_on_shutdown`.

### D6 — `queued` is acceptance, not terminal success

`queued` denotes async acceptance for delivery and SHALL NOT be interpreted or
presented (by relay, CLI, MCP, or TUI) as a terminal delivered/success outcome.
The authoritative result is the terminal outcome; when non-delivered it arrives
as a receipt. This is the normative anchor for "stop mis-acking."

### D7 — Remove the in-band caller-propagation clause from Tmux Prime Timeout

Tmux Prime Timeout currently says: "The relay worker SHALL propagate that outcome
to the MCP/CLI caller as a distinct timeout result, not collapsed into `Failed`."
Under async-only submit there is no in-band caller. The `SendOutcome::Timeout`
resolution and its distinctness stay; delivery of that result moves to the D1
receipt (timeout is non-delivered) plus the D5 floor. The clause is reworded to
state that.

## Sequencing / coupling

Resolving a wedged or timed-out head delivery *to a terminal outcome* is what
lets the per-`(namespace, runtime_directory, target_session)` unbounded mpsc
queue drain instead of head-of-line-blocking every later message to that session.
Terminal-outcome resolution drives both the queue drain and the sender receipt;
the implementation should resolve once and drive both from it.

Note a benign interaction: a receipt to `S` queues on `S`'s own delivery queue,
so a sender whose pane is itself wedged will not see receipts until it clears (or
they drop on shutdown). That is consistent with all delivery, and `relay.log`
remains the floor. The backlog is bounded, not unbounded: the per-session
delivery queue applies back-pressure, so once it fills, further receipts resolve
`Failed` and shed rather than accumulating.

## Why one change, not two

The dispatch left splitting to judgement. Splitting is not warranted: decision-
point 1 (submission-time reject) is already shipped, so there is no separable
early mis-ack fix to carve off — a `queued` that later fails can only be
corrected by the out-of-band receipt this change adds. The remaining work
(receipt envelope + `relay.log` completeness + the `queued`-is-not-success
contract + removing the sync clause) is one cohesive mechanism around a single
terminal-resolution point.

## Risks / Trade-offs

- **A receipt competes for the sender's transport.** It queues behind the
  sender's own pending deliveries and, for a wedged sender pane, waits with them.
  Bounded by best-effort delivery and the `relay.log` floor.
- **No success closure.** A sender does not get positive confirmation of a
  `delivered` outcome without consulting `relay.log`. Accepted deliberately for
  noise/cost; the failure cases are the ones that were silent and harmful.
- **CLI one-shot senders see the receipt only in `relay.log`.** A CLI invocation
  that exits after `send` returns has no live transport to receive a receipt by
  delivery time; it relies on the submission result plus `relay.log`. Long-lived
  agent senders — the review-handoff population relay/52 was about — are the
  beneficiaries.
- **Receipt rendering is per-transport work.** Each coder transport must render
  the receipt envelope; the UI path already exists. The regression test targets a
  coder transport specifically, since that is the population relay/52 was about.
