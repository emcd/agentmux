# Soul Transfer — ACP → PTY Specialist

## Identity
- Role: Pty Specialist (pivoted from ACP Specialist)
- Owner of: todos/runtime/16 — PTY migration evaluation
- Branch: `acp` (source worktree), `pty` (target worktree)
- Model: `minimax-coding-plan/MiniMax-M2.7`

## Latest Commit
- `6874cf2` — "Switch model to MiniMax-M2.7; comment out MiMo"
  - Modified: `.auxiliary/configuration/coders/opencode/settings.jsonc`
  - Co-Authored-By: MiniMax-M2.7 <no-reply@minimax.io>

## Context from Compaction Summary

### Goal
- Pivot from ACP Specialist to Pty Specialist; own tmux-to-PTY migration design (todos/runtime/16)

### Constraints & Preferences
- Design spike before code changes
- Must wait for ACP work to land before refactoring
- Factor tmux out of relay first (analogous to `src/acp` extraction) before implementing direct PTY hosting
- Research ConPTY for Windows first-class support
- Latinate naming for `ReplayEntry` variants
- Tests under `tests/` directory

### Progress
- **Configuration updates**:
  - `agentmux.toml`: session `id` `acp` → `pty`, directory → `/home/me/src/WORKTREES/agentmux/pty`, name → `MiniMax (PTY Specialist)`
  - `settings.jsonc`: active model `minimax-coding-plan/MiniMax-M2.7`
- **Nb MCP investigation**: args serialization broken for commands with args in main session; works via subagent
- todos/runtime/16 status: open (not done despite ✔️ marker)

### In Progress
- Awaiting PTY worktree creation and ACP work to land before starting design spike

### Blocked
- ACP work must land first
- Need tmux factoring before PTY implementation

### Key Decisions
- **Pivoting to Pty Specialist**: ACP transport issues + single-point-of-failure concern justified move
- **Design spike first**: compare hardened tmux guardrails vs direct PTY host MVP with complexity/risk matrix

### Next Steps
1. Resume ACP permission request end-to-end testing (unblocks when transport issues resolved)
2. Factor tmux code out of relay into `src/tmux` module (like `src/acp`)
3. Research ConPTY for Windows compatibility
4. Draft design spike comparing tmux guardrails vs direct PTY host MVP
5. Bounded per-completion eviction for `pending_tool_calls` (deferred)

### Critical Context
- **Nb MCP issue**: `nb.search`, `nb.show`, `nb.list --folder` fail with "args must be a JSON object, got string" in main session; workaround is using subagent
- todos/runtime/16 driver: recurring tmux interaction-mode edge cases (chooser/copy-mode/key_table interference) corrupting human workflows when relay injects send-keys
- PTY migration pros: full IO/focus/injection control, deterministic event model, removes tmux as external dependency, enables Windows support via ConPTY
- PTY migration cons: large scope (process supervision, terminal emulation, resize/signal handling, persistence/reconnect), higher maintenance burden

## Additional Notes

### Nb MCP Session Issue
- The main session cannot call `nb_nb` with args for commands that have a non-empty args schema (e.g., `nb.search`, `nb.show`)
- `nb.list` works because its args schema is effectively `{}` (optional folder)
- `nb.help` works fine
- Subagent works around this issue
- This appears to be a serialization issue in the `nb_nb` wrapper/tool, not a server issue

### Relevant Files
- `coordination/acp/4`: Previous ACP specialist handoff note
- `coordination/pty/1`: Current PTY specialist handoff (this file)
- `coordination/tui/1`: TUI specialist handoff
- `coordination/master/1`: Coordinator handoff
- `src/acp/`: reference implementation for factoring tmux out of relay
- `src/relay/`: where tmux factoring will begin (no `src/tmux` yet)
- `.auxiliary/instructions/practices-rust.rst`: Rust patterns to follow

### Pending Work (from ACP era)
- Permission request flow end-to-end testing
- ACP transport robustness improvements
- Any remaining ACP work to land before pivot

### OpenSpec
- AGENTS.md consulted for proposal workflow
- No active proposals currently

## Handoff Checklist
- [ ] Ensure PTY worktree is fully initialized at `/home/me/src/WORKTREES/agentmux/pty`
- [ ] Confirm `agentmux.toml` session config is correct in PTY worktree
- [ ] Verify nb tools work in new session (or document workaround)
- [ ] Review todos/runtime/16 before starting design spike
- [ ] Factor tmux from relay before implementing PTY hosting