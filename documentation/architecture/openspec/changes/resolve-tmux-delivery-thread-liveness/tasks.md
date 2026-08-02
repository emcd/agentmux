## 1. Contract

- [ ] 1.1 Add the transport-abstraction delta for observed Tmux delivery-thread
  liveness and terminal stopped-worker failures.
- [ ] 1.2 Validate the OpenSpec change and keep the existing non-blocking write
  contract intact.

## 2. Tmux Implementation

- [ ] 2.1 Retain the standard-thread `JoinHandle` beside the write sender.
- [ ] 2.2 Observe `is_finished()` before accepting writes and consume a
  finished handle without blocking.
- [ ] 2.3 Resolve post-stop `mailw` and `raww` submissions with
  `tmux_delivery_thread_stopped` while preserving the closed-channel fallback.

## 3. Coverage

- [ ] 3.1 Add a private production-path regression that leaves stale channel
  state with a finished delivery handle and asserts the next raw write future
  resolves with the stopped-thread failure.
- [ ] 3.2 Confirm normal startup, shutdown, channel-full/closed submission,
  and existing Tmux delivery behavior remain unchanged.

## 4. Verification And Handoff

- [ ] 4.1 Run focused tests, default and Pty-feature nextest, both clippy
  configurations, fmt, package, and strict OpenSpec validation.
- [ ] 4.2 Update the Pty handoff and submit the rebased stack to AuxBE.
