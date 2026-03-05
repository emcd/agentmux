## 1. Implementation

- [ ] 1.1 Create MCP tools for bundle creation, bundle reconciliation, directed
      single-target send, directed multi-target send, and broadcast send.
- [x] 1.2 Implement tmux bundle reconciliation that ensures each configured
      session exists and starts its configured coder command in the configured
      working directory, without relying on `tmux start-server` alone.
- [x] 1.3 Implement deterministic bootstrap-then-parallel reconciliation for
      missing sessions.
- [x] 1.4 Implement bounded retry with short jitter for transient tmux startup
      races.
- [x] 1.5 Mark tmuxmux-created sessions with tmux metadata and use that marker
      for ownership-aware pruning.
- [x] 1.6 Implement dedicated-socket cleanup when no tmuxmux-owned sessions
      remain.
- [x] 1.7 Implement session-target resolution to an injection pane using the
      session's currently active pane.
- [x] 1.8 Implement strict JSON envelope rendering with stable field order and
      pretty-printed formatting before `send-keys` injection.
- [x] 1.9 Implement quiescence detection before delivery with configurable quiet
      window and timeout values, defaulting to `quiet_window_ms = 750` and
      `delivery_timeout_ms = 30000`.
- [x] 1.10 Return per-target delivery results from MCP operations, including
      message identifier, target session, outcome, and failure reason when
      applicable.
- [ ] 1.11 Add configurable tmux socket selection to all tmux operations with a
      documented default.
- [ ] 1.12 Add tests for reconciliation, directed send, broadcast send,
      bootstrap-plus-parallel startup, retry/jitter behavior, ownership tagging,
      dedicated-socket cleanup, quiescence gating, timeout behavior, and
      failure reporting.
- [x] 1.13 Add user-facing documentation for quiescence behavior, including
      warning about continuously changing pane output such as clock-style
      statusline content.

## 2. Validation

- [x] 2.1 Run `cargo check --all-targets --all-features`.
- [x] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 2.3 Run `cargo test --all-features`.
