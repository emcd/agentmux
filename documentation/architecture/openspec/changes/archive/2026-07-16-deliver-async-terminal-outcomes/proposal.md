# Change: Deliver async terminal send outcomes back to the sender

## Why

`send` returns per-target `outcome = "queued"` at accept time and then tells the
sender nothing more. The real terminal outcome — `delivered`, `failed`,
`wedged` (a `failed` with `reason_code = "pane_wedged"`), `timeout`, or
`dropped_on_shutdown` — is recorded only as a `relay.send.async.completed`
inscription in `relay.log`. There is no channel back to the sender after the
ack, so `queued` is the last thing the sender hears even when delivery later
fails.

The relay/52 incident demonstrated the cost: two review handoffs to the merge
gate sat undelivered for 40 minutes and resolved as `failed` /
`dropped_on_shutdown` only at relay shutdown. Both senders saw `queued` and had
no way to learn otherwise short of a `look` at the recipient's pane. The
severity is in the silence — `queued` is success-shaped and every review handoff
rides this path.

The delivery model has two decision points, and only one is served today:

1. **Submission time — static, registry-knowable facts.** Unknown target
   (absent) and unsupported operation (present-but-incapable) already reject
   synchronously; a configured-but-offline target stays routable. This is
   already normative under the unified registry requirement and needs no change.
2. **Delivery time — dynamic outcomes only discoverable during delivery** (wedge,
   prime timeout, forwarder death, shutdown-drop). These resolve after the ack
   and never reach the sender. This is the gap.

There is no viable push channel to a coder sender today. Relay stream events
(`incoming_message`, `delivery_outcome`) are a UI/TUI-only mechanism: the
sender-directed push that already exists (`emit_sender_delivery_outcome_event`)
routes through a `session_type == Ui` gate, coder Hello binds its session type to
the bundle transport (Tmux/Acp/Pty) and never `Ui`, and the coder MCP connection
is request/response only — it does not poll for events and explicitly discards
any pushed frame. Coder agents receive information through their transport, not
through pushed stream frames.

The spec also contains an unimplemented, sync-implying clause: the Tmux Prime
Timeout requirement says the relay worker "SHALL propagate that outcome to the
MCP/CLI caller." Under async-only submit there is no in-band caller to propagate
to; the intent was always an out-of-band delivery-back that was never built.

## What Changes

- Deliver a terminal-outcome **receipt** back to the original sender as a new,
  relay-originated envelope type, routed through the **sender's own transport**
  via the existing delivery pipeline — Tmux into the pane, ACP as a turn, Pty as
  a pty write, UI as its stream frame, "each transport its own way," exactly as
  regular messages already flow. The receipt references the original
  `message_id`, the delivery target, the terminal outcome, and any
  `reason_code`.
- Send receipts for **non-delivered outcomes only** — `failed` (including
  `pane_wedged`), `timeout`, and `dropped_on_shutdown`. A `delivered` success is
  recorded in `relay.log` but not delivered back, to avoid doubling delivery
  traffic and burning the sender's attention on the common case.
- Best-effort with the same "offline is a state" routability the delivery layer
  already uses: if the sender session is not routable, the receipt is dropped,
  not persisted. `relay.log` records every terminal outcome (delivered and
  non-delivered) as the always-on floor. A CLI one-shot sender that exits after
  `send` returns has no live transport to receive a receipt and relies on the
  submission result plus `relay.log`; long-lived agent senders are the
  beneficiaries.
- Establish two invariants: a receipt is itself a delivery and SHALL NOT spawn
  its own receipt (no recursion); a receipt is relay/system-originated, not a
  peer message, so an agent can distinguish it from inbound peer traffic.
- Establish the mis-ack contract: `queued` denotes async acceptance only and
  SHALL NOT be presented as a terminal delivered/success outcome; the terminal
  outcome is authoritative and, when non-delivered, arrives as a receipt.
- Remove the sync-implying "propagate that outcome to the MCP/CLI caller" clause
  from Tmux Prime Timeout; the `SendOutcome::Timeout` resolution stays distinct
  (not collapsed into `Failed`) and — being non-delivered — is surfaced via the
  receipt.

Explicitly out of scope (do not conflate):

- **Deferred delivery / mailboxes.** Dropping the receipt when the sender is not
  routable is deliberate; store-and-forward is a separate, undelivered idea.
- **A pub/sub transport.** Undesigned.
- **Submission-time rejection.** Already delivered by the unified registry
  requirement; this change does not restate or duplicate it.
- **Success receipts.** Delivered outcomes stay log-only in this change.

## Impact

- Affected specs (`session-relay` only):
  - ADDED: Asynchronous Terminal-Outcome Receipt — the sender-directed receipt
    envelope, non-delivered outcomes only, transport-routed, best-effort/drop-if-
    absent, no-recursion, relay-as-sender, and the `queued`-is-not-terminal-
    success contract.
  - MODIFIED: Async Delivery Observability — terminal-outcome inscription covers
    the full outcome set as the floor.
  - MODIFIED: Tmux Prime Timeout — drop the in-band caller-propagation clause;
    keep the distinct-`Timeout` resolution and surface it via the receipt.
- Affected code (implementation phase; not this proposal): a new terminal-outcome
  receipt envelope/message type; the async worker's terminal-resolution path
  enqueues a receipt to the sender via the existing delivery route; the
  per-transport delivery paths render the receipt (the UI transport already emits
  the sender `delivery_outcome` frame via `emit_sender_delivery_outcome_event`;
  the coder transports gain receipt rendering); `relay.log` inscription emission;
  relay integration tests.
- The UI-only stream push stays correct for UI-class senders (an operator sending
  from the TUI) and becomes the UI transport's rendering of the receipt.

## Archive gate

Per operator constraint, this change SHALL NOT archive into the live spec ahead
of the implementation shipping. Do not archive until the sender receipt (across
the coder transports), the `relay.log` completeness, the mis-ack contract, AND
the regression test (a non-delivered outcome observable by the sender without
reading `relay.log`) are all merged — not just these proposal docs.
