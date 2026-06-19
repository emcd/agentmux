## 1. Transport contract

- [ ] 1.1 Add `mailw(envelope: DeliveryEnvelope) -> OutcomeFuture` and
      `raww(content: String, append_enter: bool) -> OutcomeFuture` to the
      `Transport` trait in `src/transports/contract.rs`
- [ ] 1.2 Add quiescence fields (`quiet_window`, `quiescence_timeout`) to
      `DeliveryEnvelope`; remove them from `DeliveryContext`; remove `n_target`
      from `DeliveryContext`
- [ ] 1.3 Remove `Transport::prepare_delivery` from the trait
- [ ] 1.4 Add `TargetConfiguration::is_transport_delivered() -> bool` (Tmux/Acp
      → true, Ui/Pubsub → false) to `src/configuration/types.rs`
- [ ] 1.5 Define `OutcomeFuture` type alias (e.g. `oneshot::Receiver<SendResult>`)
      in `src/transports/contract.rs`
- [ ] 1.6 Update `contract.rs` comment documenting the sync-movable-core
      constraint to reflect the new future-based boundary
- [ ] 1.7 Add a `max_prompt_tokens` (or equivalent) construction parameter to
      transport types that deliver to coder harnesses; thread from bundle/session
      configuration at transport construction time

## 2. Tmux transport

- [ ] 2.1 Add internal ordered channel to `TmuxTransport` carrying a
      `WriteItem` enum (`Envelope(DeliveryEnvelope, OutcomeSender)` |
      `Raw(String, bool, OutcomeSender)`)
- [ ] 2.2 Implement `mailw`: enqueue envelope into channel; return outcome
      receiver
- [ ] 2.3 Implement `raww`: enqueue raw item into channel; return outcome
      receiver (not delivered directly — preserves FIFO ordering)
- [ ] 2.4 Implement internal Tmux delivery task: drain items in FIFO order;
      accumulate contiguous `Envelope` items into a flush group; on `Raw` item,
      flush any accumulated group first (paste), then deliver the raw write;
      use head envelope's quiescence hints for the current flush group
- [ ] 2.5 Within the internal task: wait for quiescence before each flush group
      using head-envelope hints; absorb additional `Envelope` items that arrive
      during the wait into the current group
- [ ] 2.6 On shutdown signal, drain channel and resolve all pending outcome
      senders with `DroppedOnShutdown`
- [ ] 2.7 Remove `TmuxTransport::prepare_delivery` implementation
- [ ] 2.8 Remove `TmuxTransport::deliver` implementation

## 3. ACP transport

- [ ] 3.1 Add internal ordered channel to `AcpTransport` carrying the same
      `WriteItem` enum as Tmux
- [ ] 3.2 Implement `mailw`: enqueue rendered envelope into channel; return
      outcome receiver
- [ ] 3.3 Implement `raww`: enqueue raw item into channel; return outcome
      receiver (FIFO with mailw items)
- [ ] 3.4 Implement internal ACP delivery task:
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
- [ ] 3.5 Add respawn-invalidation coordination: a shutdown/respawn-signal
      channel from the ACP driver to the internal task; on respawn signal, drain
      channel and resolve all pending outcome senders with `Cancelled` before
      `release_runtime()` is called
- [ ] 3.6 On relay shutdown, drain channel and resolve all outcome senders with
      `DroppedOnShutdown`
- [ ] 3.7 Remove `AcpTransport::prepare_delivery` stub (returns ready immediately)
- [ ] 3.8 Remove `AcpTransport::deliver` implementation; `batch_envelopes`
      relay-side call and `can_take_batches` are deleted

## 4. Relay worker loop refactor

- [ ] 4.1 Replace the coalesce-hoist-drain pattern in `dispatch/worker.rs` with
      a concurrent produce-and-collect loop using `select!`: simultaneously
      drain new tasks from the relay channel (calling `mailw`/`raww` per task)
      and collect resolved outcome futures; continue until the channel is empty
      and all outcome futures have resolved
- [ ] 4.2 In the produce pass: render each task's envelope individually; call
      `mailw` or `raww` on the transport; push the returned `OutcomeFuture` onto
      a pending set
- [ ] 4.3 In the collect pass: as futures resolve, fan out `SendResult` to each
      original sender, call `note_session_served_successfully`, release pending
      slots, and record delivery inscriptions
- [ ] 4.4 Remove `classify_tmux_quiescence_hoist`, `extend_batch_with_drain`,
      and the `pre_resolved_pane` path from the worker loop; remove
      `note_tmux_delivered` from `dispatch/transport.rs`
- [ ] 4.5 Gate the dispatch on `target_member.target.is_transport_delivered()`
      instead of the `Acp/Tmux/Ui/Pubsub` match arm
- [ ] 4.6 Remove `driver.mark_busy()`, `driver.mirror_settled_readiness()`, and
      `driver.maybe_respawn_after_delivery()` from the relay worker loop (these
      relocate to the ACP transport's internal task per section 3)

## 5. Relay dispatch cleanup

- [ ] 5.1 Delete `deliver_non_ui_target_batch` and `deliver_non_ui_target` from
      `dispatch/transport.rs`
- [ ] 5.2 Delete `deliver_acp_batch_via_transport`, `deliver_acp_combined`,
      `build_tmux_envelopes`, `deliver_batch_target_tmux`,
      `deliver_one_target_tmux`, and all ACP/Tmux dispatch helpers
- [ ] 5.3 Delete `tmux_prepare_context`, `wait_error_to_send_result` (now
      transport-internal)
- [ ] 5.4 Delete `batch_envelopes` and the token-budget peel loop from
      `dispatch/orchestration.rs`; remove `PreparedBatchPayload::Batched`
      machinery
- [ ] 5.5 Delete `can_take_batches()` from `SessionType` / `TargetConfiguration`
- [ ] 5.6 Remove `DeliveryContext` quiescence and `n_target` fields from all
      construction sites

## 6. Tests and validation

- [ ] 6.1 Update or replace `coalesce_batch_tests` inline test to cover the new
      concurrent produce-and-collect loop
- [ ] 6.2 Add unit tests for `TmuxTransport::mailw` buffering and quiescence
      dispatch (mock quiescence source): verify that envelopes arriving during a
      quiescence wait are absorbed into the flush group
- [ ] 6.3 Add unit tests for `TmuxTransport` FIFO ordering: verify that a `raww`
      item causes the preceding mailw batch to flush first; verify three-batch
      scenario (N envelopes → raww → M envelopes)
- [ ] 6.4 Add unit tests for `AcpTransport::mailw` combining (mock ACP turn
      submission): verify token-budget split produces correct group boundaries
- [ ] 6.5 Add unit tests for ACP respawn-invalidation: verify that a respawn
      signal resolves all pending outcome futures with `Cancelled`
- [ ] 6.6 Confirm integration tests (send, raww, quiescence, ACP turn) remain
      green; update context construction as needed
- [ ] 6.7 Run `cargo clippy --all-targets` and `cargo test` clean
