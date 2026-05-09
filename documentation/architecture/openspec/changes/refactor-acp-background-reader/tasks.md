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

- [ ] 5.1 In `deliver_one_target_acp`: write prompt to ACP stdin, append outgoing `ReplayEntry::User` to replay buffer, return `submitted`/`accepted_in_progress` immediately on write-success
- [ ] 5.2 Set turn-complete channel in the shared reader state before writing prompt
- [ ] 5.3 Worker state transition on write: `Initializing`/`Available` → `Busy`; write failure → `Unavailable`
- [ ] 5.4 Background reader signals `Available` on turn-complete response; delivery completion event emitted at that point

## 6. Reader lifecycle and shutdown

- [ ] 6.1 On worker teardown: close shared `Arc<Mutex<ChildStdin>>` (or send sentinel), drop child process handle
- [ ] 6.2 `join` the reader thread before releasing per-session state
- [ ] 6.3 Update `unregister_worker` in `async_worker.rs` to follow the shutdown sequence

## 7. Error taxonomy

- [ ] 7.1 Add `transport_unavailable` (or `acp_child_unavailable`) error code for ACP child write failure
- [ ] 7.2 Use `transport_unavailable` instead of `internal_unexpected_failure` on stdin write I/O errors
- [ ] 7.3 Document in `src/relay/delivery/acp_delivery.rs` failure taxonomy comment

## 8. Permission event payload shapes (early — unblocks todos/mcp/24)

- [ ] 8.1 Define `permission.requested` stream event payload shape in spec delta
- [ ] 8.2 Define approve/deny request shape for relay permission resolution endpoint
- [ ] 8.3 Update `src/relay/delivery/permission_state.rs` to emit `permission.requested` from reader dispatch path (if not already done)

## 9. Integration test updates

- [ ] 9.1 Update `tests/integration/acp/lifecycle.rs` — remove drain-timing dependencies
- [ ] 9.2 Update `tests/integration/acp/look.rs` — assert look freshness from background reader
- [ ] 9.3 Update `tests/integration/acp/send.rs` — assert fire-and-forget delivery contract
- [ ] 9.4 Update `tests/integration/acp/permissions.rs` — assert permission dispatch from reader

## 10. Validation

- [ ] 10.1 `openspec validate refactor-acp-background-reader --strict`
- [ ] 10.2 Run `cargo test` integration suite
- [ ] 10.3 Manual ACP e2e: relay restart → `look` on ACP session shows history; tool call triggers permission pane
