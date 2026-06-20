# Change: Refactor transport write interface to encapsulate delivery lifecycle

## Why

The relay delivery subsystem carries transport-specific knowledge that belongs
in each transport: quiescence scheduling (`prepare_delivery`, `quiet_window`,
`quiescence_timeout`, resolved pane id in `DeliveryContext`) and batch-combining
logic (`batch_envelopes`, `can_take_batches`, relay-side ACP prompt combining)
both live relay-side today. The relay is also asymmetric in how it records
`served_successfully` — Tmux does it inside the dispatch module, ACP in the
worker loop — with no principled reason for the split. These concerns should be
fully encapsulated in each transport.

## What Changes

- **BREAKING** Replace `Transport::deliver(envelopes, context)` and
  `Transport::prepare_delivery(context)` with two non-blocking write methods:
  - `mailw(envelope) -> OutcomeFuture` — relay-wrapped message; transport
    buffers internally, waits for quiescence (if applicable), combines/pastes
    when ready, and resolves the future with the terminal outcome.
  - `raww(content, append_enter) -> OutcomeFuture` — raw input; transport
    delivers without buffering or combining.
  - `OutcomeFuture` resolves to the transport-side `SingleDeliveryOutcome`, not
    the relay `SendResult`: the transport contract never depends on
    `crate::relay`, so the relay worker maps the resolved outcome onto its own
    `SendResult` at the collect site.
- Remove quiescence fields (`quiet_window`, `quiescence_timeout`, `n_target`)
  from `DeliveryContext`; quiescence parameters move into the write envelope.
- Remove `Transport::prepare_delivery` from the trait entirely.
- Remove relay-side batch-combining logic: `batch_envelopes` relay-side call,
  `can_take_batches`, the coalesce-hoist-drain pattern, and ACP prompt-peeling
  are all deleted. Max-prompt-tokens budget moves to transport construction config.
- Remove `deliver_non_ui_target_batch`, `deliver_non_ui_target`, and the
  `TargetConfiguration::Acp/Tmux` dispatch arms in `src/relay/delivery/dispatch/`.
- **Make `Ui` and `Pubsub` first-class transports** rather than relay-internal
  delivery special-cases. The whole `Acp/Tmux/Ui/Pubsub` routing fork (and the
  `deliver_one_target_ui` / `should_route_to_ui` relay-internal path) exists only
  because `Ui`/`Pubsub` are not transports. Promote them — `UiTransport` emits a
  relay stream event via an injected broadcaster closure; `Pubsub` is
  forward-declared as a stub like `Pty` — and the worker dispatches `mailw`
  uniformly for every target. No `is_transport_delivered` capability flag is
  introduced; the routing question disappears because every target is
  transport-delivered.
- Unify `note_session_served_successfully` call site: always called from the
  worker loop for all transports; remove `note_tmux_delivered` from
  `dispatch/transport.rs`.
- Relay worker loop becomes a concurrent produce-and-collect loop (`select!`):
  simultaneously drain channel (calling `mailw`/`raww` per task, uniformly for
  every target) and collect resolved outcome futures. The loop does not block on
  pending futures before submitting new writes, preserving
  coalesce-during-quiescence-wait behavior.
- Both `mailw` and `raww` enqueue through the transport's single ordered
  internal channel. `raww` is a batch barrier: transport flushes any preceding
  `mailw` batch before delivering the raw write (FIFO across write types).
- ACP driver lifecycle calls (`mark_busy`, `mirror_settled_readiness`,
  `maybe_respawn_after_delivery`) relocate from the relay worker loop into the
  ACP transport's internal delivery task. Respawn-invalidation coordination via
  a shutdown/respawn-signal channel from the driver to the internal task.
- Reverses the `contract.rs` sync-movable-core invariant: transports now own
  internal tasks and a tokio channel; the relay↔transport boundary is
  future-based.

Each transport owns its internal ordered channel, quiescence state machine
(Tmux), combining logic and token budget (ACP), stream-broadcast delivery (Ui),
and driver lifecycle (ACP). The relay never sees pane identifiers, quiescence
waits, multi-prompt combining, or a transport-type routing fork — it submits
`mailw`/`raww` uniformly and maps each resolved `SingleDeliveryOutcome` onto its
`SendResult`.

## Impact

- Affected specs: `transport-abstraction`, `session-relay`
- Affected code:
  - `src/transports/contract.rs` — trait definition, `OutcomeFuture`,
    `DeliveryContext`, `DeliveryEnvelope`, `TransportImpl` (new `Ui`/`Pubsub`
    variants)
  - `src/relay/delivery/dispatch/` — worker loop, orchestration, transport
    dispatch module (largely deleted, including the UI short-circuit)
  - `src/tmux/transport.rs` — implement `mailw`/`raww` with internal quiescence
  - `src/acp/transport.rs` — implement `mailw`/`raww` with internal combining
  - `src/transports/ui.rs` (new) — `UiTransport` + `UiTransportServices`,
    implementing `mailw` as a stream broadcast via an injected closure
  - `src/configuration/types.rs` — remove `can_take_batches()` (no
    `is_transport_delivered()` is added)
- The ACP and Tmux `OutputView` / look path is unchanged.
- The `AcpDriverServices` injection pattern is unchanged; `UiTransportServices`
  mirrors it for the UI stream broadcaster.
- **UI delivery payload shape — RESOLVED to R1** (interim compromise toward
  option C): `DeliveryEnvelope` carries relay-populated, transport-read-only
  attribution (`sender_session`, `cc_sessions`, `authenticated_identity`;
  `on_behalf_of` deferred) so `UiTransport` builds the stream event from the
  envelope. The originally-favored lean option (b) proved incompatible with the
  committed `mailw(DeliveryEnvelope)` seam — per-message sender/cc are not on a
  lean envelope nor reconstructable inside `UiTransport`. R1 keeps the delivery
  loop uniform and preserves attribution authority (relay-populated, read-only,
  no `crate::relay` dependency); only the "lean" aesthetic bends. UI "delivery"
  == event accepted by the broadcaster (passive subscriber, no render ack). The
  clean structured-message end-state (render-in-transport, "option C") is deferred
  to todos/relay/94.
- Wire protocol is unchanged; only the relay↔transport internal interface changes.
