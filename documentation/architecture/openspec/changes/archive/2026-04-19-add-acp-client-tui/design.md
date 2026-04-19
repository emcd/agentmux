## Context
ACP agents (like opencode) expose a JSON-RPC 2.0 protocol over stdio. The relay already implements this via `AcpStdioClient` in `src/relay/delivery/acp_client.rs`. We need a standalone TUI that speaks the same protocol without the relay infrastructure, for debugging, smoke testing, and direct human interaction with ACP agents.

## Goals
- Direct ACP protocol interaction without relay, bundle config, or session management.
- Visual conversation interface (user messages / assistant responses clearly distinguished).
- Reusable by both human operators (keyboard) and MCP tools (via pty-debug).
- Shared ACP protocol code between relay delivery and client binary.

## Non-Goals
- Permission request handling (deferred — future simple choice menu).
- Multi-session management (single session per invocation).
- Relay integration or message envelope formatting.
- Persistent conversation state (session is ephemeral per invocation).

## Decisions
- **Decision**: Extract `AcpStdioClient` into `src/acp/` module rather than duplicating.
  - Rationale: relay delivery and client binary both need the same protocol. Single source of truth.
- **Decision**: Ratatui TUI rather than raw stdin/stdout.
  - Rationale: colored output, scrollable history, clean input area. Already a project dependency.
- **Decision**: Text input is always a prompt (no command prefix).
  - Rationale: simpler UX. Future commands (look, status) can be added later via keybindings or a mode prefix.
- **Decision**: Background task for ACP prompt operations.
  - Rationale: ACP prompt calls block until completion. TUI must remain responsive during streaming. Use tokio channels to push updates to the render loop.

## Architecture

```
src/
├── acp/                          (NEW — shared ACP protocol)
│   ├── mod.rs                    (re-exports)
│   └── client.rs                 (extracted from relay/delivery/acp_client.rs)
├── bin/
│   ├── agentmux.rs               (existing)
│   └── agentmux_acp.rs           (NEW — TUI binary)
├── relay/delivery/
│   └── acp_client.rs             (now imports from src/acp/)
```

### TUI Layout
```
┌──────────────────────────────────────┐
│  Session: <id>  Status: Ready        │  ← status bar (top)
├──────────────────────────────────────┤
│  [user] hello, can you help me?      │  ← scrollable history
│  [assistant] Of course! What do      │    user: different bg color
│  you need?                          │    assistant: different bg color
│                                      │
├──────────────────────────────────────┤
│  > _                                 │  ← text input (bottom)
└──────────────────────────────────────┘
```

### Interaction Flow
1. `agentmux-acp --command "opencode acp" [--session-id <id>]`
2. Binary spawns ACP agent via `sh -lc <command>`
3. Sends `initialize` → parses capabilities
4. Sends `session/new` (or `session/load` if `--session-id` provided) → gets session ID
5. Enters TUI event loop:
   - User types text + Enter → send `session/prompt`, stream response
   - Response chunks rendered as assistant messages in scrollable area
   - Ctrl+C → clean exit (close ACP child process)

### ACP Protocol Operations
- `initialize`: protocol version, client capabilities, client info
- `session/new`: create new session with working directory
- `session/load`: resume existing session by ID
- `session/prompt`: send text prompt, receive streamed response
- Session update notifications captured for snapshot display

## Risks / Trade-offs
- Extracting `AcpStdioClient` touches relay delivery code — must verify no regressions in existing relay ACP behavior.
- Ratatui event loop + async ACP operations need careful synchronization (background task + channel).
- No persistent state — conversation lost when TUI exits. Acceptable for MVP debugging/testing use case.

## Open Questions
- Should `--working-directory` be required or default to CWD?
- How should very long responses be handled in the scrollable area? (Auto-scroll to bottom vs. manual scroll lock)
