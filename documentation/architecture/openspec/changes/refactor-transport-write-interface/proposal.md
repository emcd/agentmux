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
- Remove quiescence fields (`quiet_window`, `quiescence_timeout`, `n_target`)
  from `DeliveryContext`; quiescence parameters move into the write envelope.
- Remove `Transport::prepare_delivery` from the trait entirely.
- Remove relay-side batch-combining logic: `batch_envelopes` relay-side call,
  `can_take_batches`, the coalesce-hoist-drain pattern, and ACP prompt-peeling
  are all deleted. Max-prompt-tokens budget moves to transport construction config.
- Remove `deliver_non_ui_target_batch`, `deliver_non_ui_target`, and the
  `TargetConfiguration::Acp/Tmux` dispatch arms in `src/relay/delivery/dispatch/`.
- Add `TargetConfiguration::is_transport_delivered() -> bool` capability flag to
  replace the `Ui | Pubsub` error arm in the dispatch path.
- Unify `note_session_served_successfully` call site: always called from the
  worker loop for all transports; remove `note_tmux_delivered` from
  `dispatch/transport.rs`.
- Relay worker loop becomes a concurrent produce-and-collect loop (`select!`):
  simultaneously drain channel (calling `mailw`/`raww` per task) and collect
  resolved outcome futures. The loop does not block on pending futures before
  submitting new writes, preserving coalesce-during-quiescence-wait behavior.
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
(Tmux), combining logic and token budget (ACP), and driver lifecycle (ACP). The
relay never sees pane identifiers, quiescence waits, or multi-prompt combining.

## Impact

- Affected specs: `transport-abstraction`, `session-relay`
- Affected code:
  - `src/transports/contract.rs` — trait definition, `DeliveryContext`,
    `DeliveryEnvelope`
  - `src/relay/delivery/dispatch/` — worker loop, orchestration, transport
    dispatch module (largely deleted)
  - `src/tmux/transport.rs` — implement `mailw`/`raww` with internal quiescence
  - `src/acp/transport.rs` — implement `mailw`/`raww` with internal combining
  - `src/configuration/types.rs` — add `is_transport_delivered()`
- The ACP and Tmux `OutputView` / look path is unchanged.
- The `AcpDriverServices` injection pattern is unchanged.
- Wire protocol is unchanged; only the relay↔transport internal interface changes.
