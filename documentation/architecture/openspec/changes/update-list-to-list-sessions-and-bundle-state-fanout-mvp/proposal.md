# Change: Relock list surfaces to list sessions with bundle-state fanout

## Why

Current `list` contracts are recipient-centric and do not deterministically
surface bundle up/down state with reason codes in a way that supports both
single-bundle and all-bundle operator introspection.

## What Changes

- Relock session listing surfaces:
  - CLI: `agentmux list sessions`
  - MCP: `list.sessions`
- Replace recipient-centric list payload with canonical bundle/session payload:
  - `bundle.id`, `bundle.state`, `bundle.state_reason_code?`, `bundle.sessions[]`
- Keep relay request handling single-bundle only; no relay all-bundles selector.
- Add adapter-owned all-mode fanout (`--all` / `all=true`) with deterministic
  lexicographic bundle ordering.
- Lock deterministic down-state reason codes and evidence mapping:
  - `not_started`
  - `relay_unavailable`
- Lock fallback synthesis for unreachable relay with explicit authorization
  posture:
  - home-bundle fallback allowed,
  - non-home unreachable paths fail with `relay_unavailable`.
- Keep authorization capability mapping as `list.read`.

## Breaking Changes (pre-MVP intentional)

- Remove legacy list surfaces in this change:
  - CLI `agentmux list`
  - MCP tool `list`
- Replace with:
  - CLI `agentmux list sessions`
  - MCP tool `list.sessions`

## Impact

- Affected specs:
  - `session-relay`
  - `cli-surface`
  - `mcp-tool-surface`
- Affected code (implementation follow-up):
  - relay list request/response types and handler naming
  - CLI parser/output mapping for `list sessions` and `--all`
  - MCP tool registration/handler for `list.sessions` and `all=true`
  - adapter fanout aggregation and fallback synthesis paths
