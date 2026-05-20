# Relay Module

This directory contains relay internals and the public request/response types
exported from `src/relay/mod.rs`.

## Primary Responsibilities

- Serve relay socket requests and stream-framed requests.
- Enforce authorization policy for list/send/look operations.
- Execute lifecycle transitions (`up`, `down`) per bundle.
- Route delivery across tmux and ACP transports.
- Maintain stream endpoint registration keyed by `(bundle_name, session_id)`.

## File Map

- `mod.rs`
  - relay request/response enums, shared context structs, public re-exports,
    lifecycle wrappers, and error mapping.
- `client.rs`
  - relay socket client helpers and persistent stream session request/event
    polling.
- `connection.rs`
  - relay socket serving, stream hello/request frame dispatch, hello identity
    validation, and connection write-timeout handling.
- `authorization.rs`
  - policy loading and operation-level authorization checks.
- `handlers.rs`
  - request dispatcher plus chat/look/raww handlers.
- `handlers/listing.rs`
  - lifecycle and list-session request handlers.
- `handlers/permissions.rs`
  - permission snapshot, list, and decision request handlers.
- `lifecycle.rs`
  - runtime reconcile/shutdown helpers for managed sessions.
- `stream.rs`
  - hello-frame parser, stream registry, identity collision handling, and event
    writer routing.
- `tmux.rs`
  - tmux/process adapters used by delivery and look paths.
- `delivery/`
  - transport-specific delivery decomposition:
  - `dispatch.rs`: per-target async dispatch and delivery task enqueue.
  - `async_worker.rs`: async queue worker behavior.
  - `acp_client.rs`, `acp_delivery.rs`, `acp_state.rs`: ACP lifecycle,
    prompt flow, and snapshot persistence helpers.
  - `ui_delivery.rs`: UI-stream event emission for delivery completion.
  - `results.rs`, `quiescence.rs`: shared outcome and quiescence logic.

## Runtime Behavior Notes

- Chat delivery is async-only; `delivery_mode` is no longer part of the relay
  send API. With the field removed, an internally tagged request silently
  ignores it like any other unrecognised field.
- ACP delivery supports `acp_turn_timeout_ms`; tmux delivery uses
  `quiescence_timeout_ms`.
- Pre-hello idle sockets are reaped in host connection workers to prevent
  starvation (`AGENTMUX_RELAY_PRE_HELLO_IDLE_TIMEOUT_MS` override).
- Relay-to-client writes carry a write timeout (`serve_connection`) so a
  stalled client cannot pin a connection-pool worker — or, via registered
  event writers, a delivery worker — indefinitely
  (`AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS` override, default 5s). A
  tripped write emits a `relay.connection.write_timeout` inscription.
- Host connection workers emit `relay.connection_pool.metrics` on each accept
  (`queued`/`active`/`rejected` counts) so pool saturation is observable.
- Stream events are correlated by `message_id` for send completion workflows.
