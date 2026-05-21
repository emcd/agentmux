## Why

All tmux interaction is currently embedded inside `src/relay/` with `pub(super)` visibility, making it impossible to reuse or replace cleanly. Before we can seriously evaluate native PTY support, we need a clear interface boundary between "what the relay asks for" (inject text, capture pane, resolve target, manage session lifecycle) and "how tmux implements it." This follows the same extraction pattern used for ACP in `src/acp/`.

## What Changes

- Create `src/tmux/` module (analogous to `src/acp/`) exposing module-level free functions as the public interface
- Move pane/snapshot/injection operations from `src/relay/tmux.rs` into `src/tmux/pane.rs`
- Move session lifecycle helpers from `src/relay/lifecycle.rs` into `src/tmux/lifecycle.rs`
- Keep relay's `handlers.rs` and delivery path unchanged; they become consumers of `crate::tmux::*`
- **No functional changes** to behavior — purely a refactor with no new capabilities

## Impact

- Affected specs: `session-relay` (ADDED requirement for module boundary only; no behavioral change)
- Affected code:
  - `src/relay/tmux.rs` → contents moved to `src/tmux/pane.rs`
  - `src/relay/lifecycle.rs` → tmux lifecycle helpers moved to `src/tmux/lifecycle.rs`
  - `src/relay/handlers.rs` → updated imports to use `crate::tmux::*`
  - `src/relay/delivery/dispatch.rs` → updated imports to use `crate::tmux::*`
  - `src/relay/delivery/quiescence.rs` → updated imports to use `crate::tmux::*`
- Deferred to PTY spike: traitifying the interface for PTY swap-out, PTY-native implementation