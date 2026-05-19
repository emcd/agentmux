## 1. Relay Lane (lands first)

- [x] 1.1 Delete the `delivery_mode` field from `RelayRequest::Chat`
  entirely. The relay does not reject it: with the field gone, an internally
  tagged request silently ignores it like any other unrecognised field.
- [x] 1.2 Delete `ChatDeliveryMode` enum from `src/relay.rs`.
- [x] 1.3 Remove `delivery_mode` from `RelayResponse::Chat` and
  `ChatRequestContext`.
- [x] 1.4 Remove the sync delivery loop: delete `aggregate_chat_status` and
  the `ChatDeliveryMode::Sync` match arm from `handlers.rs`. Keep
  `enqueue_sync_delivery` and `QuiescenceOptions::for_sync` — both remain in
  use by the synchronous ACP raw-write (`handle_raww`) path.
- [x] 1.5 Delete `tests/integration/relay_delivery_sync.rs`. No relay-layer
  rejection regression test is added (see Notes).
- [x] 1.6 Redesign 5 sync-mode sites in
  `tests/integration/relay_delivery_prompt.rs` to assert via the async path.
- [x] 1.7 Clean up churn in `tests/unit/relay.rs` (~12 sites),
  `relay_delivery_async.rs`, `relay_delivery_runtime.rs`,
  `session_relay_stream.rs`, `runtime_bootstrap.rs`.
- [x] 1.8 Rework the ACP integration suite for async-only delivery: convert
  `dispatch_send` in `tests/integration/acp/helpers.rs` to
  async-dispatch-then-poll (the dispatch returns `Accepted`/`Queued`, then
  blocks until the persistent worker acts); add the
  `assert_acp_delivery_unavailable` and worker-state polling helpers; convert
  the ~40 `ChatStatus::Success`/`Failure` assertion sites across
  `acp/{helpers,lifecycle,look,recovery,stop_reason,worker_state,serialization}.rs`
  to assert via worker-state polling and side effects instead.

## 2. MCP Lane (rebase onto relay)

- [x] 2.1 Remove `SendParams.delivery_mode` field and `SendDeliveryModeParam`
  enum.
- [x] 2.2 Remove `From<SendDeliveryModeParam> for ChatDeliveryMode` and the
  `ChatDeliveryMode` import.
- [x] 2.3 Remove `delivery_mode` echo from send success-response JSON and from
  the `mcp.tool.send.request` inscription.
- [ ] 2.4 Add `extra_fields` unknown-parameter rejection to `SendParams`
  (same pattern as `RawwParams`), returning `validation_invalid_params` for
  any unrecognised field. (Deferred to `todos/mcp/25`; not part of this slice.)
- [x] 2.5 Update `src/mcp/README.md` to remove `delivery_mode` reference.

## 3. CLI/TUI Lane (rebase onto relay)

- [x] 3.1 Remove `--delivery-mode` flag, `SendArguments.delivery_mode` field,
  and the send usage string entry from `src/commands/mod.rs`.
- [x] 3.2 Remove `parse_delivery_mode` and `validation_invalid_delivery_mode`
  from `src/commands/shared.rs`.
- [x] 3.3 Remove `ChatDeliveryMode::Async` construction from
  `src/tui/state/history.rs`.
- [ ] 3.4 Remove `ChatStatus` entirely: delete the enum from `src/relay.rs`,
  drop the `status` field from `RelayResponse::Chat`, strip `status` from
  `handle_chat` in `handlers.rs`, from the MCP send response JSON and
  inscription, and from the CLI send output; fix the 5 stale sync-era test
  fixtures in `mcp/send.rs` (3 tests) and `runtime_bootstrap.rs` (2 tests).
  (TUI lane; cross-lane edit authority granted for this task.)

## 4. Validation

- [x] 4.1 Run `openspec validate drop-sync-delivery --strict` and resolve all
  issues.
- [ ] 4.2 Run full test suite on each lane after its changes land; confirm
  suite green before next lane proceeds.

## Notes

- Tasks 1.1-1.8, 2.1-2.3, and 3.1-3.3 land together in one combined relay
  commit (coordinator-approved Option 1): deleting the shared
  `ChatDeliveryMode` type makes the relay, MCP, CLI, and TUI crates
  non-buildable in isolation, so the compile-coupled MCP/CLI/TUI edits ship
  with the relay slice. Tasks 2.4, 2.5, and 3.4 remain owned by the MCP and
  TUI lanes as follow-ups.
- Operator review (2026-05-18) dropped the relay-layer reject-on-present
  design: `delivery_mode` is not special-cased at the relay. With the field
  removed, an internally tagged `RelayRequest::Chat` silently ignores it like
  any other unrecognised field. The relay-layer rework that deletes the field
  is complete, and the dedicated reject test (`relay_delivery_reject_mode.rs`)
  created under the original design was removed by that rework.
- Task 2.4 (`extra_fields` rejection for `SendParams`) is deferred to
  `todos/mcp/25` rather than being folded into this slice. Pre-1.x drops do
  not require caller-facing rejection logic for the removed field; general
  unknown-field rejection is a separate hygiene improvement tracked there.
