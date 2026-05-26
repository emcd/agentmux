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
    validation, and connection write-timeout handling. One process-wide relay
    socket serves every configured bundle; each connection is bound to a
    bundle when its `hello` frame's `bundle_name` is looked up in the
    `BundleCatalog` shared with all connection workers.
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
  - `dispatch/orchestration.rs`: delivery startup, enqueue, per-target
    orchestration, and the batch-drain entry point that fans one transport
    outcome out to N coalesced tasks.
  - `dispatch/payload.rs`: envelope/raw payload preparation, UI routing, and
    the multi-envelope batch preparer that packs coalesced envelopes against
    the prompt-token budget and peels the tail when ACP cannot accept the
    rendered multi-batch result in a single dispatch.
  - `dispatch/transport.rs`: ACP/tmux transport-specific dispatch, including
    the batch variants that share one quiescence wait (tmux) or one
    permission decision window (ACP) across the coalesced tasks.
  - `dispatch/worker.rs`: per-target tokio worker task; ACP bootstrap,
    respawn, and the blocking delivery body are run on the blocking pool
    via `spawn_blocking`. Holds a carry buffer that re-queues tasks the
    coalesce loop did not accept (mode-divergent head, ACP tail peeled to
    fit the single-batch invariant).
  - `async_worker.rs`: worker registry (tokio mpsc senders) and shutdown
    drain helpers.
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
- Each connection owns a per-connection writer task (mpsc + tokio write half)
  that applies a relay-to-client write timeout
  (`AGENTMUX_RELAY_CONNECTION_WRITE_TIMEOUT_MS` override, default 5s). A
  stalled client cannot pin the writer — or, via cloned writer senders, a
  delivery worker — indefinitely: a tripped write emits a
  `relay.connection.write_timeout` inscription and the writer task exits,
  closing the channel so cloned senders surface the disconnect.
- The stream registry entry is released via a drop guard so an async-cancelled
  connection cannot leak a registry entry and wedge the next same-identity
  reconnect into an identity-claim conflict.
- Host accept loops emit `relay.connection_pool.metrics` on each accept
  (`max_connections`/`active`/`rejected` counts) so saturation against the
  unified `AGENTMUX_RELAY_MAX_CONNECTIONS` cap is observable. Connections that
  exceed the cap receive a `runtime_connection_limit_reached` error.
- Stream events are correlated by `message_id` for send completion workflows.
- Per-target delivery workers run as tokio tasks (`tokio::spawn`) reading
  from a `tokio::sync::mpsc::UnboundedReceiver`. The blocking ACP / tmux
  delivery body, the ACP single-flight prompt-completion wait, and the
  ACP bootstrap and respawn paths are offloaded to `tokio::task::spawn_blocking`
  so a tokio runtime worker thread is never pinned during transport IO.
  Worker tasks normally run on the host's main runtime; sync callers that
  enqueue work without an ambient runtime (CLI helpers, unit tests) fall
  back to a process-wide multi-thread runtime created on demand. Worker
  shutdown is observed via `shutdown_requested()` polled between receives,
  the same signal the registry-empty drain in
  `wait_for_async_delivery_shutdown` waits on.
- Each worker iteration coalesces a burst of pending envelope-mode tasks
  into one transport delivery. After the first task is received, the worker
  drains additional ready tasks via `try_recv` up to
  `AGENTMUX_RELAY_BATCH_DRAIN_MAX` (default 32), stopping at any task that
  changes payload mode or UI-routing. The non-coalescing task is pushed
  back onto a local carry buffer so it heads the next iteration without
  losing its place. For tmux envelope-mode heads the worker then hoists
  the per-target quiescence wait out of the transport: it runs
  `wait_for_quiescent_pane` on the blocking pool, and on success performs
  a second `try_recv` drain (same predicate, same drain-max budget) to
  absorb any tasks that landed in the channel while the pane wait was in
  flight. The rendered envelopes then share one paste-buffer sequence
  against the pre-resolved pane (the transport skips its own wait when a
  pre-resolved pane is supplied). ACP heads skip the hoist: the ACP single-
  flight prompt-completion wait runs *between* worker iterations, so the
  next iteration's blocking `recv` plus initial coalesce drain already
  absorb anything that queued during the previous turn. ACP-coalesced
  tasks share one `session/prompt` dispatch and one permission-decision
  window; ACP cannot accept multi-batch payloads, so when the packer
  produces more than one prompt batch the tail tasks are peeled back to
  the carry buffer to preserve the single-batch invariant. A mid-batch
  transport failure propagates the same outcome to every task in the batch
  (one delivery, one outcome by construction). When two or more tasks
  coalesce, the worker emits a `relay.chat.batch_drain.coalesced`
  inscription with the total `drained_count`, per-task `message_ids`, and
  the `pre_quiescence_count` / `post_quiescence_count` split so the
  operator can distinguish bursts already queued at batch assembly from
  bursts absorbed during the quiescence wait.
