## 1. Implementation

- [ ] 1.1 Rebase onto master
- [ ] 1.2 Create `src/tmux/` directory and `src/tmux/mod.rs` with module declarations
- [ ] 1.3 Create `src/tmux/pane.rs` with module-level free functions (pane targeting, capture, injection moved from `src/relay/tmux.rs`)
- [ ] 1.4 Create `src/tmux/lifecycle.rs` with session lifecycle free functions moved from `src/relay/lifecycle.rs`
- [ ] 1.5 Update relay imports in `src/relay/handlers.rs` to use `crate::tmux::*`
- [ ] 1.6 Update relay imports in `src/relay/delivery/dispatch.rs` to use `crate::tmux::*`
- [ ] 1.7 Update relay imports in `src/relay/delivery/quiescence.rs` to use `crate::tmux::*`
- [ ] 1.8 Remove tmux helpers from `src/relay/lifecycle.rs` (session_exists, prune_owned_session, list_owned_sessions, cleanup_tmux_server_when_unowned only; orchestration layer stays in relay/lifecycle.rs)
- [ ] 1.9 Delete `src/relay/tmux.rs`
- [ ] 1.10 Declare `pub mod tmux;` in `src/lib.rs` alongside `pub mod acp;` and `pub mod relay;`
- [ ] 1.11 Remove any `tmux` declaration from `src/relay/mod.rs`
- [ ] 1.12 Update `src/relay/delivery/mod.rs` for any tmux-related imports
- [ ] 1.13 Run unit tests under `tests/unit` and integration tests under `tests/integration`
- [ ] 1.14 Validate with `openspec validate refactor-tmux-from-relay --strict`

## 2. Deferred to PTY Spike

- [ ] 2.1 Evaluate whether a concrete `TmuxRuntime` struct should replace free functions before PTY implementation
- [ ] 2.2 Research ConPTY for Windows first-class support
- [ ] 2.3 Draft design spike comparing tmux guardrails vs direct PTY host MVP