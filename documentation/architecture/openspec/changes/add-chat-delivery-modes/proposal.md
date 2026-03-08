# Change: Add async and sync chat delivery modes

## Why

Current chat delivery is synchronous-only, which blocks MCP calls while relay
waits for quiescence. For day-to-day coordination, callers should be able to
submit messages quickly and let relay deliver when targets become ready.

## What Changes

- Add `delivery_mode` to `chat` with values `async` and `sync`.
- Default `delivery_mode` to `async` when omitted.
- Add optional `quiescence_timeout_ms` on `chat`.
- Define `async` behavior as fire-and-forget acceptance:
  - MCP returns immediately.
  - Relay queues accepted targets and waits indefinitely for quiescence before
    injection by default.
- Keep `sync` behavior for blocking delivery with current per-target outcomes
  (`delivered`, `timeout`, `failed`).
- Define mode-aware timeout defaults when `quiescence_timeout_ms` is omitted:
  - `sync`: relay-configured sync timeout.
  - `async`: no timeout (wait indefinitely).
- Define override behavior when `quiescence_timeout_ms` is provided:
  - `sync`: limit blocking wait.
  - `async`: limit background wait before dropping pending target and logging
    timeout.
- Extend MCP chat response contract to distinguish acceptance (`async`) from
  completion (`sync`).
- Keep ACK protocol out of scope.

## Impact

- Affected specs:
  - `mcp-tool-surface`
  - `session-relay`
- Affected code:
  - MCP `chat` request validation and response shaping.
  - Relay request/worker model for queued async delivery.
  - Integration tests for async acceptance and sync completion semantics.
