## Context

`list` currently behaves as a single-bundle recipient listing surface. Recent
runtime changes introduced bundle live-state information, but the machine
contract is still recipient-centric and not aligned with a future list-family
shape.

## Goals

- Provide one canonical single-bundle session-listing payload centered on
  `bundle` state and `sessions` membership.
- Support all-bundle fanout in CLI/MCP adapters without adding relay all-bundle
  request complexity in MVP.
- Keep deterministic semantics for ordering, fallback behavior, and state
  reason-code mapping.
- Preserve relay as the sole authorization decision point for relay-handled
  requests.

## Non-Goals

- No UI-client listing in bundle session lists.
- No live-presence timestamps (`last_seen`) in this change.
- No cross-bundle relay request primitive for list in MVP.

## Decisions

- Decision: relock naming to session-specific list surfaces:
  - CLI `agentmux list sessions`
  - MCP `list.sessions`
- Decision: canonical payload uses `bundle.id` naming across relay/CLI/MCP
  machine outputs.
- Decision: relay remains single-bundle for list request handling.
- Decision: all-mode fanout is adapter-owned with lexicographic bundle-id
  ordering.
- Decision: all-mode authorization is deterministic fail-fast on first
  `authorization_forbidden` and returns non-aggregate error output.
- Decision: when relay is unreachable, adapters may synthesize canonical payload
  only for requester home bundle; non-home unreachable targets fail with
  `relay_unavailable`.
- Decision: down-state reason codes in MVP are:
  - `not_started` (expected relay socket absent)
  - `relay_unavailable` (socket exists but connect/request fails)

## Risks / Trade-offs

- Renaming `list` to `list sessions`/`list.sessions` is a pre-MVP breaking
  change and will require coordinated adapter/test updates.
- Home-bundle fallback synthesis improves operator UX while intentionally not
  widening cross-bundle read behavior when relay is unavailable.
- Adapter-owned fanout can duplicate logic between CLI and MCP; this is
  acceptable in MVP to avoid relay control-plane expansion.

## Migration Plan

1. Land spec deltas for relay/CLI/MCP contracts.
2. Implement relay request/response rename and payload schema changes.
3. Implement CLI command relock (`list sessions`) and all-mode fanout.
4. Implement MCP tool relock (`list.sessions`) and all-mode fanout.
5. Update tests and docs in one sweep to avoid dual-surface drift.
