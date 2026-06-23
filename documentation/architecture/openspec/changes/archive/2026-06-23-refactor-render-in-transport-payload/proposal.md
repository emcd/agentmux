# Change: Refactor delivery payload rendering into transports

## Why

The transport-write interface made the relay worker a uniform producer, but the
payload shape remains halfway split: the worker still renders coder pane-envelope
text before calling `mailw`, while UI reads relay-populated attribution fields
from the same `DeliveryEnvelope`. This leaves rendering responsibility in the
relay delivery path and keeps `DeliveryEnvelope` as both rendered text and
structured message metadata.

## What Changes

- **BREAKING** Change `Transport::mailw` to accept a structured delivery message
  rather than pre-rendered envelope text.
- Replace `DeliveryEnvelope.rendered` with transport-safe structured fields for
  the message body, sender, target, co-recipients, timestamps, and attribution.
- Move coder pane-envelope rendering into coder transports. Tmux and ACP render
  RFC 822/MIME pane envelopes internally from the structured payload before
  paste/turn submission.
- Keep UI as a first-class transport: `UiTransport` reads the same structured
  payload directly to build `incoming_message` stream events, instead of relying
  on an R1-only body/attribution split.
- Remove relay-worker envelope rendering (`render_task_envelope`) from the
  delivery dispatch path. The relay remains authoritative for routing and
  attribution, but transports own representation-specific rendering.
- Preserve `raww` behavior as raw input, FIFO ordering, batch barriers,
  quiescence, reconnect waits, outcome futures, and transport-owned batching.

## Impact

- Affected specs: `transport-abstraction`, `pane-envelope`
- Depends on: `refactor-transport-write-interface` end state (`mailw`, `raww`,
  `DeliveryEnvelope`, `OutcomeFuture`, first-class `UiTransport`)
- Affected code:
  - `src/transports/contract.rs` — structured delivery payload type and
    `Transport::mailw` contract
  - `src/transports/ui.rs` — read structured payload directly for UI stream
    events
  - `src/tmux/transport.rs` — render pane-envelope text before tmux paste
  - `src/acp/transport.rs` — render pane-envelope text before ACP prompt
    batching/turn submission
  - `src/relay/delivery/dispatch/payload.rs` and `worker.rs` — build structured
    payloads instead of rendered envelope strings, including the
    `relay.send.envelope.metadata` `bundle_name` to `namespace` key rename
  - `src/envelope.rs` — rename envelope metadata inputs from `bundle_name` to
    `namespace`

No relay wire protocol change is intended. Send request/response shapes,
authorization, target resolution, delivery ordering, and completion event
semantics remain unchanged. The out-of-band `relay.send.envelope.metadata`
inscription is observable and intentionally renames its `bundle_name` key to
`namespace`.
