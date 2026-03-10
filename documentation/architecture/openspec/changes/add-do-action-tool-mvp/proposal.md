# Change: Add do Action Command and MCP Tool (MVP)

## Why

Operators currently have to manually monitor coder panes for context pressure
and then inject routine maintenance prompts (for example `/compact`) by hand.
Now that `look` exists, agents can detect context pressure but still lack a
safe, standardized actuator for configured maintenance actions.

## What Changes

- Add CLI action surface:
  - `agentmux do` (survey mode; list available actions for current session)
  - `agentmux do <action>` (execute mode; trigger configured action)
- Add MCP tool `do` with dynamic modes:
  - `mode=list` for action discovery
  - `mode=show` for action metadata and parameter shape
  - `mode=run` for action execution
- Add configurable action entries in `coders.toml` (kebab-case keys), so
  prompts can vary by coder while keeping action ids stable.
- Add self-target guardrails:
  - actions default to `self-only=true`
  - self-target execution is always async (no sync override)
- Defer broader authorization policy (beyond `self-only`) to follow-up work
  under the existing authorization todo/proposal track.
- Define standard execution envelope/inscriptions for `do` action dispatch.

## Non-Goals (MVP)

- Generic `agentmux help` subcommand/tool for all dynamic surfaces.
- Cross-bundle or cross-user action execution.
- Arbitrary free-form prompt injection outside configured action allowlist.

## Impact

- Affected specs:
  - `cli-surface`
  - `mcp-tool-surface`
  - `session-relay`
- Affected code:
  - command parsing/execution (`src/commands.rs`)
  - MCP tool surface (`src/mcp/mod.rs`)
  - relay request handling for action dispatch
  - configuration parsing for action registry
