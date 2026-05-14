# Change: Refactor ACP I/O model to persistent background reader

## Why

The ACP client (`src/acp/client.rs`) uses a synchronous, request-scoped I/O
model: ACP stdout is only read while the relay is inside an active `request()`
call. Between calls the pipe is unread. A post-response drain window (200–250 ms)
compensates but is fundamentally insufficient.

Consequences of the current model:

- `look` on an ACP session shows no history after relay restart: `session/load`
  reply content is missed if the drain window closes before OpenCode streams it.
- `session/request_permission` events that arrive between requests are silently
  dropped, breaking the permission flow.
- The worker thread blocks for the entire prompt turn (potentially minutes),
  preventing it from accepting new delivery tasks or routing other events.

## What Changes

- **ADDED** Persistent background reader thread per ACP session (one per child
  process) that owns the ACP stdout fd, reads continuously using
  `epoll`/`select`/`poll`, and dispatches by JSON-RPC message type:
  - `session/update` → append to shared in-memory replay buffer
  - `session/request_permission` → enqueue permission request, emit stream event
  - Response matching pending prompt request-id → signal turn completion
  - Other notifications → log and discard

- **ADDED** Stdin writer serialization via `Arc<Mutex<ChildStdin>>` shared
  across all write contexts (delivery thread, permission resolution, shutdown).

- **ADDED** Pending-request registry (`HashMap<u64, oneshot::Sender<Value>>`)
  owned by the background reader; routes responses to waiting callers for
  non-prompt requests (`initialize`, `session/new`, `session/load`) that still
  require synchronous acknowledgment.

- **MODIFIED** Prompt dispatch is now fire-and-forget: write to ACP stdin
  succeeds → relay returns `submitted`/`accepted_in_progress` immediately,
  without waiting for `session/update` or any other first-activity signal.
  Delivery completion is emitted by the background reader when the prompt
  response arrives. Wire-level phase tokens (`accepted_in_progress`,
  `accepted_dispatched`) are unchanged.

- **MODIFIED** Replay buffer is updated on send: when the relay writes a user
  prompt to ACP stdin, it appends the outgoing message as a `ReplayEntry::User`
  immediately so `look` reflects the submitted message before any response.

- **MODIFIED** Worker readiness transitions:
  - Prompt write success → `busy` (replaces "first activity observed")
  - Background reader observes terminal `stopReason` → `available`
  - Reader thread exit (any cause) or stdin write failure → `unavailable`

- **MODIFIED** `LiveBuffer` and `acp_stream_stalled` freshness vocabulary
  acquires new semantics under continuous-reader model:
  - `LiveBuffer`: "background reader thread is alive and feeding updates"
    (was: "served from in-memory after request-scoped drain")
  - `acp_stream_stalled`: "reader alive but observed N seconds of silence"
    (was: "drain window saw nothing")
  Wire tokens (`Fresh`/`Stale`, `LiveBuffer`/`None`) are unchanged.

- **ADDED** Dedicated error code `transport_unavailable` (or
  `acp_child_unavailable`) for ACP child process write failure, distinguishable
  from `internal_unexpected_failure`.

- **REMOVED** Post-response drain constants
  (`ACP_LOAD_POST_RESPONSE_DRAIN_TIMEOUT`, `ACP_PROMPT_POST_RESPONSE_DRAIN_TIMEOUT`)
  and `drain_post_response_notifications` function — dead code under
  continuous-reader model.

- **ADDED** Reader lifecycle/shutdown sequence: close child stdin → drop child
  process handle → `join` reader thread → release per-session state.

## Impact

- Affected specs: `acp-client`, `session-relay`
- Affected code (implementation):
  - `src/acp/client.rs` — core rewrite of I/O model
  - `src/relay/delivery/acp_delivery.rs` — fire-and-forget dispatch; worker state transitions
  - `src/relay/delivery/async_worker.rs` — background reader spawn/shutdown lifecycle
  - `src/relay/delivery/acp_state.rs` — remove drain timeout constants (dead code)
  - `src/relay/handlers.rs` — verify `handle_look` snapshot derivation post-change
  - `tests/integration/acp/{lifecycle,look,send,permissions}.rs` — adjust for drain-timing removal

## Sequencing

This change MUST land before `coordination/acp/3` (ACP snapshot freshness
token contract). `coordination/acp/3` cuts disk persistence in favor of
in-memory replay; this change is what makes the in-memory replay buffer
continuously populated. Landing `coordination/acp/3` first creates a regression
window where `look` reads a buffer only populated during request scopes.

## Note reference

Background: `agentmux:coordination/relay/8` — proposal text, relay specialist
review (2026-05-09), mcp specialist review (2026-05-09).
