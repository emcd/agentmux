# Change: Replace client-class model with session-type taxonomy

## Why

`add-mcp-permission-decision-surface` introduced `RelayClientClass`
(`Agent` / `Ui` / `Operator`) and an `operator-class` policy flag to gate who
may decide ACP permission requests. Post-merge review found this conflates two
unrelated concerns:

1. **Authorization** — already fully expressed by the `authorize_grant` policy
   capability (`grant` control).
2. **Transport role** — an intrinsic property of how a session is wired, not a
   privilege granted at connect time.

The client-class model also blocks embeddability: rust-litrpg player agents
receive envelopes as prompts and make in-process tool calls — "agent vs UI"
cannot express this transport; behavior must be derivable from config alone.

## What Changes

- **BREAKING** — `hello` frame drops `client_class`; remaining fields:
  `bundle_name`, `session_id`, `schema_version`.
- **BREAKING** — all relay response/event fields carrying session identity now
  emit `session@bundle` canonical form (e.g., `"master@agentmux"`).
- Delete `RelayClientClass` / `RelayStreamClientClass` enums and all
  `client_class` machinery from relay, MCP, and TUI.
- Introduce session-type taxonomy: `{tmux, acp, ui, pubsub}` declared by
  exactly one subtable per `[[sessions]]` config entry.
- Relay routes and delivers based on session type from config, not hello
  `client_class`.
- Permission-decision authority gated by `authorize_grant` capability alone;
  `UI-Mediated Decision Submitter Gate` and `operator-class` policy control
  removed.
- Hello hydrates `session_id@bundle_name` canonical identity; unified lookup
  across bundle members and `users.toml` global users (identified by
  `@GLOBAL` suffix on `session_id`).
- Rename `tui.toml` → `users.toml`; global user IDs in `session@GLOBAL` form.
- `ui` and `pubsub` session types recognized from day one; empty subtable
  body valid; unimplemented types fail fast with a structured error rather
  than a parse failure.

## Impact

- Affected specs: `session-relay` (major), `runtime-bootstrap` (moderate),
  `tui-surface` (minor), `mcp-tool-surface` (minor)
- Affected code: `src/relay.rs`, `src/relay/**`, `src/configuration.rs`,
  `src/runtime/**`, `src/mcp/mod.rs`, `src/tui/state/mod.rs`, test fixtures
- **Prerequisite**: archive `add-mcp-permission-decision-surface` before
  applying the `mcp-tool-surface` delta in this change. The grant tool
  requirements must be present in the base spec for the MODIFIED delta to
  apply cleanly at archive time.
