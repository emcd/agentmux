# Change: Add raww direct write surface (MVP)

## Why

Operators need a direct write path to one target session without dropping into
tmux. Existing `send` is envelope delivery between sessions; it is not a
single-target direct input operation.
This is especially important for slash-command style interactions that cannot
be invoked through envelope delivery semantics.

## What Changes

- Add relay operation contract for direct write: `raww`.
- Add CLI surface:
  - `agentmux raww <target-session> --text <text> [--no-enter]`
- Add MCP surface:
  - top-level tool `raww`
- Lock deterministic same-bundle MVP behavior for raww.
- Lock canonical validation/error taxonomy and acceptance payload schema.
- Add relay authorization control `raww` and denial capability label
  `raww.write`.
- Lock transport mapping:
  - tmux: literal write (+ Enter by default; optional opt-out)
  - acp: existing shared worker path via `session/prompt`

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
  - `tui-surface`
- Affected code (implementation follow-up):
  - relay request/validation/authorization/transport paths for raww
  - policy parsing/validation for `raww` control
  - MCP tool wiring for `raww`
  - CLI command wiring for `raww`
  - TUI raw write dispatch integration against relay raww contract
