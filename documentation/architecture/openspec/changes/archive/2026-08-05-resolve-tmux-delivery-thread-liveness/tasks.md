## 1. Contract

- [x] 1.1 Add the transport-abstraction delta for observed Tmux delivery-thread
  liveness and terminal stopped-worker failures.
- [x] 1.2 Validate the OpenSpec change and keep the existing non-blocking write
  contract intact.

## 2. Tmux Implementation

- [x] 2.1 Retain the standard-thread `JoinHandle` beside the write sender.
- [x] 2.2 Observe `is_finished()` before accepting writes and consume a
  finished handle without blocking.
- [x] 2.3 Resolve post-stop `mailw` and `raww` submissions with
  `tmux_delivery_thread_stopped` while preserving the closed-channel fallback.

## 3. Coverage

- [x] 3.1 Add a private production-path regression that leaves stale channel
  state with a finished delivery handle and asserts the next raw write future
  resolves with the stopped-thread failure.
- [x] 3.2 Confirm normal startup, shutdown, channel-full/closed submission,
  and existing Tmux delivery behavior remain unchanged.

## 4. Verification And Handoff

- [x] 4.1 Run focused tests, default and Pty-feature nextest, both clippy
  configurations, fmt, package, and strict OpenSpec validation. Re-confirmed
  by Coordinator on merged master 2026-08-05: focused Tmux nextest 16/16
  (including the stopped-thread regression), pty-feature clippy `-D warnings`
  clean, `cargo package --list --allow-dirty` clean, full default nextest
  869/869, `openspec validate --all --strict` 31/31.
- [x] 4.2 Update the Pty handoff and submit the rebased stack to AuxBE. The
  Pty handoff (`coordination/tmux/1`) was updated at each checkpoint. A
  standalone AuxBE submission for this fix specifically was never sent — the
  fix (`3854f7b`, rebased as `bae2763`) was folded into `backend`'s lineage
  ahead of the `establish-delivery-commit-contract` Tier B work before that
  submission happened, and AuxBE's five-round empty-gating-list approval of
  the full `backend` branch (verdict at `fca0af7`, merged to master
  byte-identical at `a5025fa`/`c149f61`) covers this diff as part of that
  larger review, not as a distinct pass. Recorded here rather than silently
  checked off: if a future audit wants a fix reviewed as its own unit, this
  one did not get that.
