# Relay Module

This directory documents relay internals beyond the public request/response
types in `src/relay.rs`.

## Primary Responsibilities

- Serve relay socket requests and stream-framed requests.
- Enforce authorization policy for list/send/look operations.
- Execute lifecycle transitions (`up`, `down`) per bundle.
- Route delivery across tmux and ACP transports.
- Maintain stream endpoint registration keyed by `(bundle_name, session_id)`.

## File Map

- `src/relay.rs`
  - relay request/response enums and main connection handling entrypoints.
  - owns stream hello/request frame dispatch and error mapping.
- `authorization.rs`
  - policy loading and operation-level authorization checks.
- `handlers.rs`
  - request handlers for list/look/chat/lifecycle operations.
- `lifecycle.rs`
  - runtime reconcile/shutdown helpers for managed sessions.
- `stream.rs`
  - hello-frame parser, stream registry, identity collision handling, and event
    writer routing.
- `tmux.rs`
  - tmux/process adapters used by delivery and look paths.
- `delivery/`
  - transport-specific delivery decomposition:
  - `dispatch.rs`: per-target dispatch + status aggregation.
  - `async_worker.rs`: async queue worker behavior.
  - `acp_client.rs`, `acp_delivery.rs`, `acp_state.rs`: ACP lifecycle,
    prompt flow, and snapshot persistence helpers.
  - `ui_delivery.rs`: UI-stream event emission for delivery completion.
  - `results.rs`, `quiescence.rs`: shared outcome and quiescence logic.

## Runtime Behavior Notes

- ACP sync delivery supports `acp_turn_timeout_ms`; tmux delivery uses
  `quiescence_timeout_ms`.
- Pre-hello idle sockets are reaped in host connection workers to prevent
  starvation (`AGENTMUX_RELAY_PRE_HELLO_IDLE_TIMEOUT_MS` override).
- Stream events are correlated by `message_id` for send completion workflows.
