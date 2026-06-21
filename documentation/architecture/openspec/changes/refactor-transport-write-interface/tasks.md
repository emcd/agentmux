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
      senders with `DroppedOnShutdown`. (Section 5 conformance fix: the three
      tmux shutdown-drop sites emitted `SendOutcome::Failed` + reason_code
      `dropped_on_shutdown`; corrected to `SendOutcome::DroppedOnShutdown` so the
      worker — which now reports the transport's outcome faithfully — surfaces the
      `dropped_on_shutdown` outcome the relay shutdown taxonomy expects. The ACP
      equivalent (`dropped_on_shutdown_outcome`, task 3.6) still emits
      `SendOutcome::Failed`; latent, not exercised by a test — flagged for the ACP
      lane.)
- [ ] 2.7 Remove `TmuxTransport::prepare_delivery` implementation (deferred:
      still present; lands when Section 5 removes the last relay `deliver`/
      `prepare_delivery` callsite — was checked prematurely)
- [ ] 2.8 Remove `TmuxTransport::deliver` implementation (deferred: still
      present; lands with the Section 5/6 callsite removal — was checked
      prematurely)

## 3. ACP transport

- [x] 3.1 Add internal ordered channel to `AcpTransport` carrying the same
      `WriteItem` enum as Tmux
- [x] 3.2 Implement `mailw`: enqueue rendered envelope into channel; return
      outcome receiver
- [x] 3.3 Implement `raww`: enqueue raw item into channel; return outcome
      receiver (FIFO with mailw items)
- [ ] 3.4 Implement internal ACP delivery task. Done: drain buffer; combine
      contiguous `Envelope` items into one combined prompt respecting the token
      budget (excess remains in the channel for the next turn); flush the
      envelope group before a `Raw` item, then submit the raw content as its own
      turn; fan the turn outcome to all outcome senders for the submitted group;
      set the transport-internal `shared.readiness` (Busy on dispatch, settled
      after turn). NOT done (was checked prematurely; reopened under Resolution B
      — `relay` lane): the readiness transitions are not mirrored to the relay
      global registry, and respawn is not driven. Section 3 originally specified
      `driver.mark_busy()` / `driver.mirror_settled_readiness()` /
      `driver.maybe_respawn_after_delivery()` calls, but the internal task cannot
      call driver methods; the actual mechanism is (a) inject a `MirrorStateFn`
      into the task so it mirrors every readiness transition globally itself, and
      (b) drive respawn off the existing `respawn_needed` watch via a driver-owned
      async path (the signal is currently emitted but never consumed). Tracked as
      tasks 5.6/5.7 below.
- [x] 3.5 Add respawn-invalidation coordination: a shutdown/respawn-signal
      channel from the ACP driver to the internal task; on respawn signal, drain
      channel and resolve all pending outcome senders with `Cancelled` before
      `release_runtime()` is called
- [x] 3.6 On relay shutdown, drain channel and resolve all outcome senders with
      `DroppedOnShutdown`
- [ ] 3.7 Remove `AcpTransport::prepare_delivery` stub (deferred: still present;
      lands with the Section 5/6 callsite removal — was checked prematurely)
- [ ] 3.8 Remove `AcpTransport::deliver` implementation; `batch_envelopes`
      relay-side call and `can_take_batches` are deleted (deferred: `deliver`
      still present and `batch_envelopes`/`can_take_batches` still used by the
      relay batch path; lands with Section 5/6 — was checked prematurely)

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

- [x] 5.1 Replace the coalesce-hoist-drain pattern in `dispatch/worker.rs` with
      a concurrent produce-and-collect loop using `select!` (produce arm:
      `receiver.recv()`; collect arm: `JoinSet::join_next()` over the in-flight
      write outcomes; plus a shutdown-poll tick). Calls `mailw`/`raww` per task,
      **uniformly for every target — no transport-type gate**; the loop runs
      until the senders drop and the in-flight set drains. (`JoinSet` is the
      tokio-native pending set; the crate has no `futures`/`FuturesUnordered`
      dependency.)
- [x] 5.2 In the produce pass (`submit_task`/`prepare_coder_write`): render each
      coder task's envelope individually via `render_task_envelope` (or submit the
      raw body via `raww`); call `mailw`/`raww`; spawn a `JoinSet` collector that
      awaits the returned `OutcomeFuture`. Tmux `startup()` is run lazily on first
      coder task (it needs that task's resolved `BundleMember`); ACP started in
      `bootstrap()`.
- [x] 5.3 In the collect pass (`collect_outcome`): map each
      `SingleDeliveryOutcome` onto `SendResult` (`outcome_to_send_result`), fan out
      via `complete_task_outcome` (which emits the sender outcome event + the
      `relay.send.async.completed` inscription), call
      `note_session_served_successfully` for delivered coder writes, and release
      the pending slot.
- [x] 5.4 Removed `classify_tmux_quiescence_hoist`, `extend_batch_with_drain`,
      `coalesce_batch`/`can_coalesce_with_head`, the carry buffer, and the
      `pre_resolved_pane` path from the worker loop; `note_tmux_delivered`
      removed along with the whole `dispatch/transport.rs` (deleted).
- [x] 5.5 The relay worker loop no longer calls `driver.mark_busy()`,
      `driver.mirror_settled_readiness()`, or `driver.maybe_respawn_after_delivery()`
      (those were already retired in the Section 3 ACP fixup); the loop body has no
      `TransportImpl::Acp` match — it is fully transport-agnostic.
- [x] 5.6 ACP global readiness mirror relocated into the internal delivery task
      (done in the Section 3 ACP fixup, commit c838997): the task holds the
      `ReadinessMirror` on `AcpSharedState` and mirrors every Busy/settled
      transition to the relay global registry itself.
- [x] 5.7 ACP respawn driving relocated off the worker (done in the Section 3 ACP
      fixup, commit c838997): a driver-owned async respawn monitor consumes the
      transport's stable `respawn_needed` watch over `Arc<Mutex<AcpTransport>>`.

## 6. Relay dispatch cleanup

Section 5 stranded most of these as dead `pub(super)` helpers (clippy `-D
warnings`), so the forced subset (6.1-6.4, 6.7) landed with Section 5. The
remainder (6.5, 6.6, plus the legacy `deliver`/`prepare_delivery` trait-method
removals deferred under 1.2/1.3/2.7/2.8/3.7/3.8) is a pure-removal follow-up: the
trait/enum methods are `pub` so they raise no dead-code warning, and `tmux`'s
internal delivery task still shares `wait_for_quiescent_pane` with the legacy
`deliver`, so deleting the seam is a coherent separate commit.

- [x] 6.1 Deleted `deliver_non_ui_target_batch` and `deliver_non_ui_target`
      (the whole `dispatch/transport.rs` was deleted).
- [x] 6.2 Deleted `deliver_acp_batch_via_transport`, `deliver_acp_combined`,
      `build_tmux_envelopes`, `deliver_batch_target_tmux`,
      `deliver_one_target_tmux`, and the rest of the ACP/Tmux dispatch helpers
      (all in the deleted `dispatch/transport.rs`).
- [x] 6.3 Deleted `tmux_prepare_context`, `wait_error_to_send_result` (the
      transport's internal task now owns the quiescence wait + its error mapping).
- [x] 6.4 Deleted the relay-side token-budget peel loop and `PreparedBatchPayload`
      (`prepare_batch_delivery_payload`) from the dispatch payload path, plus the
      now-dead per-task `AsyncDeliveryTask.batch_settings` field (budget reads at
      transport construction time per 1.6). NOTE: the `batch_envelopes` function
      itself is retained — it is now consumed by the ACP transport's internal
      delivery task to combine a contiguous envelope group into one turn.
- [ ] 6.5 Delete `can_take_batches()` from `SessionType` / `TargetConfiguration`
      (still present; `pub`, no dead-code warning — folds into the seam-removal
      commit).
- [ ] 6.6 Remove `DeliveryContext` quiescence and `n_target` fields from all
      construction sites (deferred with the `deliver`/`prepare_delivery` removal —
      `DeliveryContext` is still the legacy seam's parameter).
- [x] 6.7 `deliver_one_target_ui` and `should_route_to_ui` were already removed in
      Section 4 (UI delivery flows through `UiTransport::mailw`); only doc-comment
      references remain. No `Ui`/`Pubsub` short-circuit remains in the dispatch path.

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
