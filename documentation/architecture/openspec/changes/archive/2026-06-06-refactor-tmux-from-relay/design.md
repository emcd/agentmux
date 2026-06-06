## Context

All tmux interaction lives in `src/relay/` today:

- `src/relay/tmux.rs` (`pub(super)`) — pane targeting, snapshot capture, literal text injection, quiescence diagnostics
- `src/relay/lifecycle.rs` — session reconcile/shutdown, calls `run_tmux_command` and `run_tmux_command_capture` directly

Consumers:

- `src/relay/handlers.rs` — look path calls `capture_pane_tail_lines`, `resolve_active_pane_target`
- `src/relay/delivery/dispatch.rs` — delivery path calls `inject_prompt`, `inject_literal_text`, `resolve_active_pane_target`
- `src/relay/delivery/quiescence.rs` — calls `capture_pane_snapshot`, `resolve_window_activity_marker`, `operator_interaction_active`, `resolve_cursor_column`

ACP precedent (`src/acp/`): module-level free functions, not a struct.

## Goals / Non-Goals

- Goals:
  - Extract tmux code into `src/tmux/` with clean public interface
  - Relay becomes a consumer of `crate::tmux::*` — no logic change in relay handlers or delivery
  - Make tmux socket path configurable at the interface boundary (already is, via `&Path`)
  - Preserve all function signatures; only visibility and module location change
  - Lay groundwork for PTY evaluation without committing to PTY implementation

- Non-Goals:
  - Do NOT traitify the interface in this change (deferred to PTY spike)
  - Do NOT implement PTY hosting in this change
  - Do NOT change any runtime behavior — purely mechanical module relocation

## Decisions

### Module structure

```
src/tmux/
  mod.rs          # public re-exports + module declarations
  pane.rs         # pane targeting, capture, injection (module-level free functions)
  lifecycle.rs    # session lifecycle helpers (module-level free functions)
```

### Interface type: module-level free functions

Decision: Use module-level free functions rather than a concrete struct. Matches ACP precedent (`src/acp/`). Concrete struct deferred until PTY swap-out is evaluated and interface shape is better understood.

Public interface (free functions in `src/tmux/pane.rs`):

- `pub fn resolve_active_pane_target(tmux_socket: &Path, target_session: &str) -> Result<String, String>`
- `pub fn capture_pane_snapshot(tmux_socket: &Path, pane_target: &str) -> Result<String, String>`
- `pub fn capture_pane_tail_lines(tmux_socket: &Path, pane_target: &str, requested_lines: usize) -> Result<Vec<String>, String>`
- `pub fn inject_literal_text(tmux_socket: &Path, pane_target: &str, text: &str, append_enter: bool) -> Result<(), String>`
- `pub fn inject_prompt(tmux_socket: &Path, pane_target: &str, prompt: &str) -> Result<(), String>`
- `pub fn resolve_cursor_column(tmux_socket: &Path, pane_target: &str) -> Result<usize, String>`
- `pub fn resolve_window_activity_marker(tmux_socket: &Path, pane_target: &str) -> Result<Option<String>, String>`
- `pub fn operator_interaction_active(tmux_socket: &Path, target_session: &str, pane_target: &str) -> Result<Option<String>, String>`
- `pub fn sanitize_diagnostic_text(text: &str) -> String`
- `pub fn emit_delivery_diagnostic(event: &str, details: &Value)`

Public interface (free functions in `src/tmux/lifecycle.rs`):

- `pub fn session_exists(tmux_socket: &Path, session_name: &str) -> Result<bool, String>`
- `pub fn create_member_once(tmux_socket: &Path, member: &BundleMember, start_command: &str) -> Result<(), String>`
- `pub fn prune_owned_session(tmux_socket: &Path, session_name: &str) -> Result<(), RelayError>`
- `pub fn list_owned_sessions(tmux_socket: &Path) -> Result<Vec<String>, RelayError>`
- `pub fn cleanup_tmux_server_when_unowned(tmux_socket: &Path) -> Result<bool, RelayError>`

### What stays in relay

`src/relay/handlers.rs`, `src/relay/delivery/dispatch.rs`, and `src/relay/delivery/quiescence.rs` remain as-is structurally; only `use` imports change to reference `crate::tmux::*`.

### Visibility

`src/tmux/` is fully `pub` — any future consumer (MCP, CLI, PTY host) can depend on it. `pub(super)` visibility is removed.

### run_tmux_command / run_tmux_command_capture

These remain private helpers inside `src/tmux/` (not part of the public interface). They were never intended to be relay's external contract.

## Risks / Trade-offs

- Risk: Mechanical refactor with no functional change may introduce subtle bugs (moved code paths). Mitigation: test coverage via existing integration tests.
- Trade-off: Free functions vs struct — free functions are simpler and match ACP precedent; concrete struct deferred until PTY evaluation.

## Migration Plan

1. Create `src/tmux/` directory and scaffold files
2. Move pane.rs content (copy from relay/tmux.rs, update module references)
3. Move lifecycle.rs content (copy tmux lifecycle functions from relay/lifecycle.rs)
4. Update relay imports to use `crate::tmux::*`
5. Delete originals from `src/relay/`
6. Run existing tests — all must pass
7. Validate with `openspec validate refactor-tmux-from-relay --strict`

Rollback: git revert the change if tests fail; no spec updates needed (ADDED delta only, no MODIFIED).

## Open Questions

- Deferred to PTY spike:
  - Should the interface be traitified before or after PTY evaluation begins?
  - Should we keep lifecycle ownership in `tmux` module or extract it further?
  - Should a concrete `TmuxRuntime` struct be introduced when PTY swap-out is evaluated?