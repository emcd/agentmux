## Why

`TmuxTransport` currently treats the presence of its channel sender as proof
that the detached delivery thread is alive. If that thread exits while the
receiver remains open, later `mailw` and `raww` calls can enqueue work with no
consumer and leave their outcome futures pending indefinitely.

## What Changes

- Retain and observe the Tmux delivery thread handle instead of using
  `sender.is_some()` as its liveness signal.
- Detect a stopped delivery thread before accepting new writes.
- Resolve writes submitted after thread termination with a terminal failure
  outcome rather than parking them on an unobserved channel.
- Add regression coverage that stops the delivery thread and verifies the
  next write future resolves.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `transport-abstraction`: strengthen synchronous delivery completion so a
  stopped Tmux delivery thread cannot leave an outcome future unresolved.

## Impact

- Changes `src/tmux/transport.rs` to retain a standard-thread join handle and
  report stopped-worker writes as terminal failures.
- Adds focused Tmux unit coverage.
- Adds no new dependency, configuration key, or public API.
