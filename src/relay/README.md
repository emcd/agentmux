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
  - public re-export hub plus relay entrypoint wrappers.
- `contract.rs`
  - relay request/response enums and public payload structs.
- `context.rs`
  - internal request context and delivery task structs shared across relay
    submodules.
- `constants.rs`
  - relay-local constants shared across submodules.
- `identity.rs`
  - canonical/bare session identity helpers.
- `errors.rs`
  - relay error constructors and configuration error mapping.
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
  - `dispatch/mod.rs`: delivery dispatch re-export hub.
  - `dispatch/orchestration.rs`: delivery startup, enqueue, and per-target
    orchestration.
  - `dispatch/payload.rs`: envelope/raw payload preparation and UI routing.
  - `dispatch/transport.rs`: ACP/tmux transport-specific dispatch.
  - `dispatch/worker.rs`: async worker loop and ACP respawn handling.
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
