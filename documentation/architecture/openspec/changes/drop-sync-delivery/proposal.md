# Change: Drop Sync Delivery Mode

## Why

`delivery_mode = sync` blocks the relay handler thread waiting for quiescence
on each target in sequence, then returns a timeout error that resembles a
delivery failure. The root problem is not a slow quiescence wait — blocking,
caller-visible completion is the wrong abstraction for an async multi-agent
relay. External callers (LitRPG integration team) encountered misleading
timeouts and filed `issues/relay/14`, now closed as moot.

Keeping `delivery_mode = async` as a valid no-op after removing `sync` would
invite the same confusion in a different form. Removing the field entirely is
the only clean state.

## What Changes

- `delivery_mode` is removed as an accepted field from the relay send API.
  Any request that includes `delivery_mode` — including `delivery_mode = async`
  — is rejected with `validation_invalid_params` and a details object
  identifying the removed field.
- `ChatDeliveryMode` enum is deleted. No one-variant stub is kept.
- `delivery_mode` is removed from `RelayRequest::Chat`, `RelayResponse::Chat`,
  and `ChatRequestContext`.
- The sync delivery loop (`enqueue_sync_delivery`), quiescence defaults table
  (`quiet_window_ms = 750`, `delivery_timeout_ms = 30000`), sync-specific
  scenarios, and sync response shape (`success`, `partial`, `failure`) are
  removed.
- MCP `SendParams` gains an `extra_fields` unknown-parameter rejection pattern
  (same pattern used by `RawwParams`) so MCP callers who supply `delivery_mode`
  receive `validation_invalid_params` rather than silent ignore.
- `quiescence_timeout_ms` is retained as the sole async tmux quiescence knob.

## Rejection Details Pattern

Relay rejection SHALL use a details object to aid external-client diagnosis:

```
validation_invalid_params
details: { field: "delivery_mode", reason: "removed; async is the only delivery mode" }
```

This follows the existing `validation_invalid_timeout_field_for_transport`
details pattern.

## serde Implementation Note

`RelayRequest` uses `#[serde(tag = "operation")]`. serde does not support
`deny_unknown_fields` on internally tagged enum variants. Deleting the
`delivery_mode` field alone results in silent ignore. The relay lane must
replace the field with an explicit capture and fail-fast when present:

```rust
#[serde(default)]
delivery_mode: Option<serde_json::Value>,
```

## Lane Sequencing

Relay must land its struct and enum changes before mcp and cli slices can
build. mcp and cli rebase onto the relay commit.

## Impact

- Affected specs:
  - `session-relay`
  - `mcp-tool-surface`
  - `cli-surface`
- Affected code (implementation follow-up):
  - relay: `src/relay.rs`, `src/relay/handlers.rs`,
    `src/relay/delivery/dispatch.rs`
  - mcp: `src/mcp/mod.rs`, `src/mcp/README.md`
  - cli: `src/commands/send.rs`, `src/commands/mod.rs`,
    `src/commands/shared.rs`
  - tui: `src/tui/state/history.rs`
  - tests: `tests/integration/relay_delivery_sync.rs` (delete/repurpose),
    `tests/integration/relay_delivery_prompt.rs` (5 sync sites redesign),
    `tests/integration/acp/helpers.rs` (drop delivery_mode param),
    and churn across unit and integration test files
