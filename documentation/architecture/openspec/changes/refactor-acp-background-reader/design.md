## Context

The ACP client uses stdio JSON-RPC 2.0 to communicate with an OpenCode ACP
server child process. The current I/O model is synchronous and request-scoped:
reads happen only inside `request()` calls. A post-response drain loop
compensates but loses notifications sent outside the call window.

This design covers the refactor to a persistent background reader thread,
fire-and-forget prompt dispatch, and the associated invariants.

Stakeholders: relay specialist (implementer), mcp specialist (permission event
consumer surface), coordinator (proposal author).

## Goals / Non-Goals

Goals:
- Read ACP stdout continuously, for the lifetime of the session
- Populate the in-memory replay buffer from all `session/update` notifications,
  not just those received during an active `request()` call
- Dispatch `session/request_permission` events from the background reader
- Return `submitted` immediately on prompt write-success; emit completion async
- Preserve non-prompt request/response pairing (initialize, session/new,
  session/load still need synchronous ack)

Non-Goals:
- ACP network transport (stdio-only in scope)
- ACP protocol version changes
- Multi-session per child process (each child hosts one session in current use)
- Full async/tokio refactor (background thread using blocking I/O is sufficient)

## Decisions

### Background reader as an OS thread (not tokio task)

`AcpStdioClient` uses blocking `BufReader<ChildStdout>`. Spawning an OS thread
that owns the blocking fd is simpler than introducing async I/O or a reactor.
The thread sleeps on `read_line` and wakes on data. No epoll boilerplate needed
for a single fd — `read_line` blocks until data or EOF, which is the desired
behavior.

Alternatives considered: tokio `AsyncBufReader` (requires tokio runtime
at the call site and changes the error propagation model across the crate).

### Stdin writer: `Arc<Mutex<ChildStdin>>`

Multiple write contexts exist: delivery thread (prompt/session/new/session/load/
initialize), permission resolution (JSON-RPC response to `session/request_
permission`), and shutdown/cleanup. A shared `Arc<Mutex<ChildStdin>>` serializes
these. A dedicated writer channel (mpsc + writer task) was considered but adds
allocation and latency with no benefit for this low-rate use case.

### Pending-request registry: `HashMap<u64, oneshot::Sender<Value>>`

The background reader owns the registry. Before a non-prompt write, the caller
registers its request-id and waits on a oneshot channel. The reader looks up
the id in the registry and sends the response value. Timeout is the caller's
responsibility (retain existing timeout semantics for initialize/load/new).

For `session/prompt`, the caller registers no oneshot; the reader dispatches
the response to a separate `Option<turn_complete_sender>` field set by the
delivery thread before the write. This avoids coupling the prompt path to the
registry contract.

### Fire-and-forget prompt dispatch

Write to stdin → success → return `submitted`. The kernel pipe buffer (typically
64 KB on Linux) guarantees the child will read the written bytes. Waiting for
first `session/update` adds latency and creates an edge case if OpenCode never
sends one (e.g., refusal with no streaming).

### Reader thread exit as primary error signal

If the child process dies or the stdout fd is closed, `read_line` returns EOF.
This is the canonical failure signal from the reader side. On reader exit
(any cause including panic), the worker transitions to `Unavailable` and all
pending oneshot channels receive an error.

## Risks / Trade-offs

- **Reader thread panic**: mitigated by `thread::Builder::spawn` + catching
  panics or using `catch_unwind` in the reader loop. Worker transitions to
  `Unavailable` on join with `Err`.
- **Stdin write ordering**: `Arc<Mutex<ChildStdin>>` solves this but introduces
  a lock acquisition on every write. Acceptable: writes are low-rate (one per
  prompt turn or permission event).
- **Pending-request registry cleanup**: if a non-prompt caller times out and
  drops its receiver, the registry entry leaks until the reader sends the
  response and finds a closed channel. Safe (channel send returns `Err`, entry
  is cleaned up at that point or on session teardown).

## Migration Plan

1. Implement background reader and pending-request registry in `src/acp/client.rs`
   behind the same `AcpStdioClient` interface.
2. Switch delivery to fire-and-forget (`deliver_one_target_acp` returns on
   write-success).
3. Remove drain loop and drain constants.
4. Update worker state transitions in `acp_delivery.rs`.
5. Update permission dispatch path in `permission_state.rs`.
6. Update integration tests (remove drain-timing dependencies).
7. Validate with `openspec validate refactor-acp-background-reader --strict`.

## Open Questions

None remaining after relay + mcp specialist reviews (2026-05-09). All six relay
gaps and six mcp flags are addressed in proposal.md §§5–13 and this design doc.
