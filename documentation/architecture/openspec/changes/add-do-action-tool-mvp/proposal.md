# Change: Add do Action Command and MCP Tool (MVP)

## Why

Operators currently have to manually monitor coder panes for context pressure
and then inject routine maintenance prompts (for example `/compact`) by hand.
Now that `look` exists, agents can detect context pressure but still lack a
safe, standardized actuator for configured maintenance actions.

## What Changes

- Add CLI action surface:
  - `agentmux do` (list mode; list available actions for current session)
  - `agentmux do --show <action>` (show mode; return metadata for one action)
  - `agentmux do <action>` (execute mode; trigger configured action)
- Add MCP tool `do` with dynamic modes:
  - `mode=list` for action discovery
  - `mode=show` for action metadata and parameter shape
  - `mode=run` for action execution
  - no target selector fields in MVP (`do run` is self-target by contract)
- Add configurable action entries at canonical path
  `[[coders.do-actions]]` in `coders.toml` (kebab-case keys), so prompts can
  vary by coder while keeping action ids stable.
- Add self-target guardrails:
  - `do run` is self-target only in MVP (no target selector fields)
  - action entries default to `self-only=true`
  - self-target execution is always async (no sync override)
- Defer broader authorization policy (beyond `self-only`) to follow-up work
  under the existing authorization todo/proposal track.
- Define standard execution envelope/inscriptions for `do` action dispatch.
- Lock one canonical `do run` acceptance payload shape across relay/CLI/MCP.

## Non-Goals (MVP)

- Generic `agentmux help` subcommand/tool for all dynamic surfaces.
- Cross-bundle or cross-user action execution.
- Arbitrary free-form prompt injection outside configured action allowlist.
- Targeted/non-self `do run` execution (deferred until post-MVP authorization
  and targeting contract work).

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
