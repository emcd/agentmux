## Context

`TmuxTransport` owns a bounded channel and a detached standard-thread delivery
worker. The transport currently retains only the channel sender, so the sender's
presence is mistaken for worker liveness. A worker that exits can leave the
transport accepting writes even though no task can resolve their outcome
futures.

The transport contract already requires every write future to resolve. This
change closes the implementation gap for the Tmux worker without changing the
relay write API or introducing a cross-thread callback.

## Goals / Non-Goals

**Goals:**

- Observe the Tmux delivery thread's actual liveness.
- Reject writes after an observed worker stop with a terminal failure outcome.
- Preserve the existing non-blocking `mailw`/`raww` API and channel ordering.
- Exercise the stopped-worker path with a private production-path test.

**Non-Goals:**

- Automatically restart a stopped delivery thread.
- Change the worker's normal delivery, batching, quiescence, or shutdown logic.
- Redesign delivery ownership across ACP, Pty, or UI transports.
- Diagnose or recover the original cause of a worker panic or early return.

## Decisions

### Retain a standard-thread join handle

Store the `JoinHandle<()>` returned by `thread::spawn` beside the channel
sender. `ensure_task_running` checks `is_finished()` before treating the
transport as running. Once finished, it takes and joins the handle, clears the
stale sender, and returns a stopped-worker error. Joining only after
`is_finished()` avoids blocking the caller while still consuming the handle and
observing a panic or normal return.

### Fail closed instead of restarting

The startup context is consumed when the worker is first started, and a worker
stop has no safe generic restart contract. Subsequent writes therefore resolve
immediately with `SendOutcome::Failed` and reason code
`tmux_delivery_thread_stopped`. This is observable and bounded, unlike keeping
the stale sender or attempting an incomplete restart.

### Keep the existing channel-closed path

`enqueue` continues to resolve `Full` and `Closed` send errors immediately.
Handle observation adds an earlier, explicit liveness check; it does not remove
the channel error fallback for a race between the check and the send.

## Risks / Trade-offs

- [Worker panic remains a failure] The fix does not recover a panic or early
  return → it converts later writes into an explicit terminal failure and
  preserves the panic's existing process diagnostics.
- [No automatic recovery] A stopped worker remains unavailable until the
  owning transport is restarted → this avoids rebuilding consumed startup
  context and silently changing delivery ownership.
- [Small race after the check] The worker can stop immediately after
  `is_finished()` returns false → the existing `try_send` closed-channel path
  still resolves that write.

## Migration Plan

No configuration, persistence, or API migration is required. The new reason
code is emitted only when a previously started Tmux delivery thread has
stopped; normal startup and channel-full behavior are unchanged.

## Open Questions

None.
