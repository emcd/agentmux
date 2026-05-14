## 1. Drain window and draining accessor removal

- [x] 1.1 Remove `ACP_LOAD_POST_RESPONSE_DRAIN_TIMEOUT` and `ACP_PROMPT_POST_RESPONSE_DRAIN_TIMEOUT` constants from `src/relay/delivery/acp_state.rs` (or wherever they live)
- [x] 1.2 Remove `drain_post_response_notifications` function from `src/acp/client.rs`
- [x] 1.3 Remove all call sites of the drain function
- [x] 1.4 Remove `take_replay_entries` (draining accessor) from `src/acp/client.rs`
- [x] 1.5 Update `agentmux-acp` debug TUI binary to use the cursor accessor instead of the draining accessor; thread a `usize` cursor through the render loop

## 2. Stdin writer serialization

- [x] 2.1 Wrap `ChildStdin` in `Arc<Mutex<ChildStdin>>` inside `AcpStdioClient`
- [x] 2.2 Update all existing stdin write paths to acquire the mutex before writing
- [x] 2.3 Pass the shared `Arc<Mutex<ChildStdin>>` to the background reader for permission response writes

## 3. Pending-request registry

- [x] 3.1 Add `pending_requests: Arc<Mutex<HashMap<u64, oneshot::Sender<serde_json::Value>>>>` to reader state
- [x] 3.2 Expose `register_pending(request_id) -> oneshot::Receiver<Value>` for non-prompt callers
- [x] 3.3 Background reader looks up `id` in registry on any JSON-RPC response and sends on the oneshot channel
- [x] 3.4 On reader exit, drain registry and send `Err` on all pending receivers

## 4. Background reader thread

- [x] 4.1 Spawn reader thread in `AcpStdioClient` after child process is started
- [x] 4.2 Reader loop: `read_line` on `BufReader<ChildStdout>` → parse JSON-RPC message
- [x] 4.3 Dispatch by message type:
  - `session/update` notification → parse `ReplayEntry` items, append to shared replay buffer, update `last_acp_frame_observed_at`
  - `session/request_permission` request → call permission dispatch path; write JSON-RPC response to stdin via shared writer
  - Response matching turn-complete sender → signal turn completion, transition worker to `Available`
  - Response matching pending registry entry → send on oneshot
  - Other notifications → log via inscription, discard
- [x] 4.4 On reader loop exit (EOF or I/O error): transition worker state to `Unavailable`; drain pending-request registry with error; close turn-complete sender if set
- [x] 4.5 Store reader `JoinHandle` in `AcpStdioClient` for lifecycle management

## 5. Fire-and-forget prompt dispatch

- [x] 5.1 In `deliver_one_target_acp`: write prompt to ACP stdin, return `submitted`/`accepted_in_progress` immediately on write-success; on write-success, also append a `ReplayEntry::User` for the prompt text to the shared replay buffer so `look` reflects the submitted message before any `session/update` arrives (implemented inside `AcpStdioClient::prompt` so all `prompt()` callers — relay delivery and the `agentmux_acp` debug binary — get the invariant)
- [x] 5.2 Set turn-complete channel in the shared reader state before writing prompt
- [x] 5.3 Worker state transition on write: `Initializing`/`Available` → `Busy`; write failure → `Unavailable`
- [x] 5.4 Background reader signals `Available` on turn-complete response; delivery completion event emitted at that point

  Design refinements (aligned with Eric 2026-05-09; design note at
  `agentmux:decisions/2`):

  - **D2** ACP is single-batch only; `deliver_one_target_acp` `debug_assert`s
    `prompt_batches.len() == 1`. Multi-batch is a tmux concept and is not
    plumbed for ACP.
  - **D3** No runtime invalidation flag. `*acp_runtime = None` lines removed
    (request-scoped vestige). The persistent worker stays alive with its
    `AcpStdioClient`; subsequent sends to an `Unavailable` worker fail-fast
    via a pre-task state check (`get_acp_worker_state`).
  - **D4** No relay-side turn timeout for `prompt()`. Long agent prompts
    (model thinking, retries) are first-class; agent death is detected via
    bg-reader EOF (`PromptCompletion::ConnectionClosed`). `request()`
    (initialize/load/new) retains its timeout. Vestigial timeout code is
    `#[allow(dead_code)]` with TODO markers; full cleanup is a separate
    follow-up once the `acp_turn_timeout_ms` MCP param is dropped.
  - **D8 (deferred)** Worker auto-respawn (worker exits on `Unavailable`,
    registry GCs entry, next `try_existing_worker` respawns fresh child) was
    designed but deferred. Filed as `agentmux:todos/acp/14`. The pre-task
    fail-fast preserves the existing integration-test contract; auto-respawn
    will replace it in the follow-up.

  Per-target ACP single-flight is preserved by a new
  `AcpStdioClient::wait_for_prompt_complete()` method: the worker thread
  blocks on a dropped-sender signal between tasks so the next dispatch
  cannot violate the "one in-flight prompt per session" invariant.

## 6. Reader lifecycle and shutdown

- [x] 6.1 On worker teardown: close shared `Arc<Mutex<ChildStdin>>` (or send sentinel), drop child process handle
- [x] 6.2 `join` the reader thread before releasing per-session state
- [x] 6.3 Update `unregister_worker` in `async_worker.rs` to follow the shutdown sequence

  Implementation note: the worker thread owns the `AcpStdioClient` (in `acp_runtime`)
  and exits when the registry's sender channel disconnects. `unregister_worker`
  removes the registry entry, dropping the sender; on the worker side, `recv`
  returns `Disconnected` and the loop breaks, dropping `acp_runtime`. The new
  `AcpStdioClient::Drop` (via `shutdown()`) kills the child, waits for it,
  and joins the reader thread before returning. No additional change to
  `unregister_worker` is needed — the cascade is correct as-is.

## 7. Error taxonomy

- [x] 7.1 Add `transport_unavailable` (or `acp_child_unavailable`) error code for ACP child write failure
- [x] 7.2 Use `transport_unavailable` instead of `internal_unexpected_failure` on stdin write I/O errors
- [x] 7.3 Document in `src/relay/delivery/acp_delivery.rs` failure taxonomy comment

## 9. Integration test updates

- [x] 9.1 Update `tests/integration/acp/lifecycle.rs` — remove drain-timing dependencies

  Drain timings were already removed in slice A. Lifecycle tests
  (`acp_disconnect_after_first_activity_preserves_accepted_response`,
  `acp_disconnect_before_first_activity_does_not_block_sync_dispatch_ack`,
  `acp_send_uses_persisted_session_id_when_config_id_is_absent`) all pass
  with the C-5 contract (sync returns `accepted_in_progress`; subsequent
  sends after `Unavailable` fail-fast).
- [x] 9.2 Update `tests/integration/acp/look.rs` — assert look freshness from background reader

  Done in slice C-1: worker registry now stores the bg reader's
  `Arc<Mutex<Vec<ReplayEntry>>>` directly, so `get_acp_worker_snapshot`
  always returns live state. `acp_look_captures_updates_emitted_after_prompt_response`
  un-ignored. The remaining ignored test
  `acp_look_marks_snapshot_stale_when_updates_are_stalled` is gated on
  snapshot timestamp tracking (separate work item beyond this change).
- [x] 9.3 Update `tests/integration/acp/send.rs` — assert fire-and-forget delivery contract

  All existing send tests pass under the new contract. The "Delivered"
  outcome with `delivery_phase: "accepted_in_progress"` is now the
  synchronous return for sync sends; terminal completion arrives via the
  stream (already covered by `relay_async_chat_emits_terminal_delivery_outcome_to_sender_ui_stream`).

- [x] 9.4 Update `tests/integration/acp/permissions.rs` — assert permission dispatch from reader

  Permission tests already validate dispatch from the background reader
  (`acp_request_permission_keeps_worker_busy_while_pending_decision`).
  The repurposed `acp_worker_state_stays_available_after_protocol_error`
  validates the new "logical error from agent keeps worker healthy"
  semantic.

## 10. Validation

- [ ] 10.1 `openspec validate refactor-acp-background-reader --strict`
- [ ] 10.2 Run `cargo test` integration suite
- [ ] 10.3 Manual ACP e2e: relay restart → `look` on ACP session shows history; tool call triggers permission pane
