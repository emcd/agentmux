## 1. Relay Lane (lands first)

- [ ] 1.1 Replace `delivery_mode` field in `RelayRequest::Chat` with explicit
  reject-on-present check (`Option<serde_json::Value>`); return
  `validation_invalid_params` with details object when `Some`.
- [ ] 1.2 Delete `ChatDeliveryMode` enum from `src/relay.rs`.
- [ ] 1.3 Remove `delivery_mode` from `RelayResponse::Chat` and
  `ChatRequestContext`.
- [ ] 1.4 Remove sync delivery loop: `enqueue_sync_delivery`,
  `QuiescenceOptions::for_sync`, `aggregate_chat_status`, and the
  `ChatDeliveryMode::Sync` match arm from `handlers.rs`.
- [ ] 1.5 Delete `tests/integration/relay_delivery_sync.rs`; add rejection
  regression test asserting `validation_invalid_params` when `delivery_mode`
  is present.
- [ ] 1.6 Redesign 5 sync-mode sites in
  `tests/integration/relay_delivery_prompt.rs` to assert via the async path.
- [ ] 1.7 Clean up churn in `tests/unit/relay.rs` (~12 sites),
  `relay_delivery_async.rs`, `relay_delivery_runtime.rs`,
  `session_relay_stream.rs`, `runtime_bootstrap.rs`.
- [ ] 1.8 Drop `delivery_mode` param from
  `tests/integration/acp/helpers.rs::dispatch_send_with_mode` and update
  callers in `acp/look.rs` and `acp/worker_state.rs`.

## 2. MCP Lane (rebase onto relay)

- [ ] 2.1 Remove `SendParams.delivery_mode` field and `SendDeliveryModeParam`
  enum.
- [ ] 2.2 Remove `From<SendDeliveryModeParam> for ChatDeliveryMode` and the
  `ChatDeliveryMode` import.
- [ ] 2.3 Remove `delivery_mode` echo from send success-response JSON and from
  the `mcp.tool.send.request` inscription.
- [ ] 2.4 Add `extra_fields` unknown-parameter rejection to `SendParams`
  (same pattern as `RawwParams`), returning `validation_invalid_params` for
  any unrecognised field. (Overlaps `todos/mcp/25`; fold into this slice.)
- [ ] 2.5 Update `src/mcp/README.md` to remove `delivery_mode` reference.

## 3. CLI/TUI Lane (rebase onto relay)

- [ ] 3.1 Remove `--delivery-mode` flag, `SendArguments.delivery_mode` field,
  and the send usage string entry from `src/commands/mod.rs`.
- [ ] 3.2 Remove `parse_delivery_mode` and `validation_invalid_delivery_mode`
  from `src/commands/shared.rs`.
- [ ] 3.3 Remove `ChatDeliveryMode::Async` construction from
  `src/tui/state/history.rs:30`.

## 4. Validation

- [ ] 4.1 Run `openspec validate drop-sync-delivery --strict` and resolve all
  issues.
- [ ] 4.2 Run full test suite on each lane after its changes land; confirm
  suite green before next lane proceeds.
