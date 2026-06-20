## 1. Transport contract

- [x] 1.1 Add `mailw(envelope: DeliveryEnvelope) -> OutcomeFuture` and
      `raww(content: String, append_enter: bool) -> OutcomeFuture` to the
      `Transport` trait in `src/transports/contract.rs` (additions-only: default
      `unimplemented!` stub bodies; each transport overrides in sections 2-4)
- [ ] 1.2 Add quiescence fields (`quiet_window`, `quiescence_timeout`) to
      `DeliveryEnvelope`; remove them from `DeliveryContext`; remove `n_target`
      from `DeliveryContext` (deferred: removals land with sections 5-6)
- [ ] 1.3 Remove `Transport::prepare_delivery` from the trait (deferred: lands
      with the last callsite removal in sections 5-6)
- [x] 1.4 Define `OutcomeFuture` type alias in `src/transports/contract.rs`
      (`oneshot::Receiver<SingleDeliveryOutcome>` — the transport-side outcome,
      not the relay `SendResult`, to preserve transports' no-relay-dependency
      invariant; the worker maps it onto `SendResult` at the collect site)
- [x] 1.5 Update `contract.rs` comment documenting the sync-movable-core
      constraint to reflect the new future-based boundary
- [x] 1.6 Add a `max_prompt_tokens` construction parameter to transport types
      that deliver to coder harnesses; thread from session configuration at
      transport construction time (stored on `TmuxTransport`/`AcpWorkerDriver`
      with accessors; consumed by the internal delivery task in sections 2-3)

## 2. Tmux transport

- [x] 2.1 Add internal ordered channel to `TmuxTransport` carrying a
      `WriteItem` enum (`Envelope(DeliveryEnvelope, OutcomeSender)` |
      `Raw(String, bool, OutcomeSender)`)
- [x] 2.2 Implement `mailw`: enqueue envelope into channel; return outcome
      receiver
- [x] 2.3 Implement `raww`: enqueue raw item into channel; return outcome
      receiver (not delivered directly — preserves FIFO ordering)
- [x] 2.4 Implement internal Tmux delivery task: drain items in FIFO order;
      accumulate contiguous `Envelope` items into a flush group; on `Raw` item,
      flush any accumulated group first (paste), then deliver the raw write;
      use head envelope's quiescence hints for the current flush group
- [x] 2.5 Within the internal task: wait for quiescence before each flush group
      using head-envelope hints; absorb additional `Envelope` items that arrive
      during the wait into the current group
- [x] 2.6 On shutdown signal, drain channel and resolve all pending outcome
      senders with `DroppedOnShutdown`
- [x] 2.7 Remove `TmuxTransport::prepare_delivery` implementation
- [x] 2.8 Remove `TmuxTransport::deliver` implementation

## 3. ACP transport

- [x] 3.1 Add internal ordered channel to `AcpTransport` carrying the same
      `WriteItem` enum as Tmux
- [x] 3.2 Implement `mailw`: enqueue rendered envelope into channel; return
      outcome receiver
- [x] 3.3 Implement `raww`: enqueue raw item into channel; return outcome
      receiver (FIFO with mailw items)
- [x] 3.4 Implement internal ACP delivery task:
      - drain buffer (blocking recv, wait for ≥1 item)
      - call `driver.mark_busy()` before turn submission
      - for contiguous `Envelope` items: concatenate into one combined prompt
        respecting the configured token budget (call `batch_envelopes` from
        `crate::envelope`); excess items remain in the channel for the next turn
      - for `Raw` items: flush any accumulated envelope group first, then
        submit the raw content as its own turn
      - submit turn; call `driver.mirror_settled_readiness()` after
      - call `driver.maybe_respawn_after_delivery(outcome)` keyed off turn result
      - fan turn outcome to all outcome senders for the submitted group
- [x] 3.5 Add respawn-invalidation coordination: a shutdown/respawn-signal
      channel from the ACP driver to the internal task; on respawn signal, drain
      channel and resolve all pending outcome senders with `Cancelled` before
      `release_runtime()` is called
- [x] 3.6 On relay shutdown, drain channel and resolve all outcome senders with
      `DroppedOnShutdown`
- [x] 3.7 Remove `AcpTransport::prepare_delivery` stub (returns ready immediately)
- [x] 3.8 Remove `AcpTransport::deliver` implementation; `batch_envelopes`
      relay-side call and `can_take_batches` are deleted

## 4. Ui / Pubsub transports

Finishes the decoupling intent: `Ui` and `Pubsub` stop being relay-internal
delivery special-cases and become first-class transports. The relay worker then
dispatches `mailw` uniformly for every target, eliminating the
`Acp/Tmux/Ui/Pubsub` routing branch (and the `is_transport_delivered` predicate
it would otherwise require).

- [x] 4.1 Add `UiTransport` implementing `Transport` in its own module
      (`src/transports/ui.rs`): `mailw` emits the message as a relay stream event
      via an injected broadcaster closure and resolves the `OutcomeFuture` after a
      bounded UI-reconnect wait (no quiescence/combining/token budget; the
      reconnect wait runs on its own thread so `mailw` stays non-blocking)
- [x] 4.2 Add `UiTransportServices` carrying the injected `Arc<dyn Fn>` stream
      broadcaster (`broadcast_incoming`) plus the `delivery_outcome` phase emitter
      (`emit_phase`), constructed relay-side (`build_ui_transport_services`)
      closing over the relay stream — no `crate::relay` import (mirrors
      `AcpDriverServices`)
- [x] 4.3 `UiTransport::raww` resolves its future with an unsupported/failed
      outcome (UI is not raw-writable); `give_output` returns `None` (UI is not
      lookable); `is_ready` returns true
- [x] 4.4 Add `TransportImpl::Ui(UiTransport)` variant; forward-declare
      `TransportImpl::Pubsub` as a stub variant (like `Pty`) until the Pubsub
      transport lands (capability rows + delegate arms for both)
- [x] 4.5 Construct `UiTransport` for `Ui` targets in the worker: non-ACP
      workers resolve their transport kind from the first head task
      (`task_routes_to_ui`) and swap to `UiTransport`, threading the UI services;
      Pubsub stub variant added (no Pubsub targets route yet)
- [x] 4.6 UI delivery payload shape — RESOLVED to R1 (interim compromise toward
      option C). `DeliveryEnvelope` carries relay-populated, transport-read-only
      attribution (`sender_session`, `cc_sessions`, `authenticated_identity`;
      `on_behalf_of` deferred unless the task carries it) so `UiTransport` builds
      the `RelayStreamEvent` from the envelope. Chosen because lean-(b) is
      incompatible with the committed `mailw(DeliveryEnvelope)` seam: sender/cc are
      per-message and not reconstructable inside `UiTransport` (the injected
      broadcaster closes over the target, not the per-message sender/cc), so
      `UiTransport::mailw` cannot build the event from a lean envelope. R1 keeps
      the worker dumb and the delivery loop uniform, and preserves FE's substantive
      requirements — attribution stays relay-authoritative, transport-read-only,
      and carries no `crate::relay` dependency (plain owned data); only the "lean"
      aesthetic bends. The fields are shaped to migrate into option C cleanly.
      Spec note: UI "delivery" == event accepted by the broadcaster (the TUI is a
      passive subscriber; no per-recipient render ack), so `UiTransport`'s
      `SingleDeliveryOutcome` is success-on-broadcast, not confirmed-rendered. The
      clean structured-message end-state (render-in-transport, "option C") is
      deferred to todos/relay/94.

## 5. Relay worker loop refactor

- [ ] 5.1 Replace the coalesce-hoist-drain pattern in `dispatch/worker.rs` with
      a concurrent produce-and-collect loop using `select!`: simultaneously
      drain new tasks from the relay channel (calling `mailw`/`raww` per task,
      **uniformly for every target — no transport-type gate**) and collect
      resolved outcome futures; continue until the channel is empty and all
      outcome futures have resolved
- [ ] 5.2 In the produce pass: render each task's payload individually; call
      `mailw` or `raww` on the transport; push the returned `OutcomeFuture` onto
      a pending set
- [ ] 5.3 In the collect pass: as futures resolve, map each
      `SingleDeliveryOutcome` onto `SendResult`, fan out to each original sender,
      call `note_session_served_successfully`, release pending slots, and record
      delivery inscriptions
- [ ] 5.4 Remove `classify_tmux_quiescence_hoist`, `extend_batch_with_drain`,
      and the `pre_resolved_pane` path from the worker loop; remove
      `note_tmux_delivered` from `dispatch/transport.rs`
- [ ] 5.5 Remove `driver.mark_busy()`, `driver.mirror_settled_readiness()`, and
      `driver.maybe_respawn_after_delivery()` from the relay worker loop (these
      relocate to the ACP transport's internal task per section 3)

## 6. Relay dispatch cleanup

- [ ] 6.1 Delete `deliver_non_ui_target_batch` and `deliver_non_ui_target` from
      `dispatch/transport.rs`
- [ ] 6.2 Delete `deliver_acp_batch_via_transport`, `deliver_acp_combined`,
      `build_tmux_envelopes`, `deliver_batch_target_tmux`,
      `deliver_one_target_tmux`, and all ACP/Tmux dispatch helpers
- [ ] 6.3 Delete `tmux_prepare_context`, `wait_error_to_send_result` (now
      transport-internal)
- [ ] 6.4 Delete `batch_envelopes` and the token-budget peel loop from
      `dispatch/orchestration.rs`; remove `PreparedBatchPayload::Batched`
      machinery
- [ ] 6.5 Delete `can_take_batches()` from `SessionType` / `TargetConfiguration`
- [ ] 6.6 Remove `DeliveryContext` quiescence and `n_target` fields from all
      construction sites
- [ ] 6.7 Delete `deliver_one_target_ui` and `should_route_to_ui`; UI delivery
      now flows through `UiTransport::mailw`, so no `Ui`/`Pubsub` short-circuit
      remains in the dispatch path

## 7. Tests and validation

- [ ] 7.1 Update or replace `coalesce_batch_tests` inline test to cover the new
      concurrent produce-and-collect loop
- [ ] 7.2 Add unit tests for `TmuxTransport::mailw` buffering and quiescence
      dispatch (mock quiescence source): verify that envelopes arriving during a
      quiescence wait are absorbed into the flush group
- [ ] 7.3 Add unit tests for `TmuxTransport` FIFO ordering: verify that a `raww`
      item causes the preceding mailw batch to flush first; verify three-batch
      scenario (N envelopes → raww → M envelopes)
- [ ] 7.4 Add unit tests for `AcpTransport::mailw` combining (mock ACP turn
      submission): verify token-budget split produces correct group boundaries
- [ ] 7.5 Add unit tests for ACP respawn-invalidation: verify that a respawn
      signal resolves all pending outcome futures with `Cancelled`
- [x] 7.6 Add unit tests for `UiTransport::mailw` (`tests/unit/ui_transport.rs`):
      verify it broadcasts a stream event via the injected services closure and
      resolves the outcome future; verify the bounded reconnect wait resolves
      `Timeout` when no UI connects; verify `raww` resolves unsupported and
      `give_output` is `None`
- [ ] 7.7 Confirm integration tests (send, raww, quiescence, ACP turn, UI
      delivery) remain green; update context construction as needed
- [ ] 7.8 Run `cargo clippy --all-targets` and `cargo test` clean
