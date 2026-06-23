# Design: Render delivery payloads in transports

## Context

`refactor-transport-write-interface` moved delivery scheduling, quiescence,
batching, and UI fan-out behind `Transport::mailw`/`raww`. It intentionally left
payload rendering in an interim R1 shape: coder deliveries receive pre-rendered
pane-envelope text in `DeliveryEnvelope.rendered`, while `UiTransport` reads
relay-populated attribution fields from the same envelope to build stream events.

The desired Option C end state is to pass structured message data through
`mailw`. Each transport then renders the representation it owns: Tmux and ACP
render pane-envelope text, while UI renders stream events.

## Goals / Non-Goals

- Goals: make `DeliveryEnvelope` a structured transport-neutral message; move
  pane-envelope rendering out of the relay worker; preserve relay authority over
  attribution and routing; keep UI, ACP, and Tmux delivery behavior unchanged at
  their external boundaries.
- Non-Goals: changing send wire contracts, adding new transport types, changing
  ACP turn semantics, changing tmux quiescence semantics, changing UI delivery
  acknowledgement semantics, or redesigning raw input delivery.

## Decisions

### Decision: `DeliveryEnvelope` becomes structured message data

`DeliveryEnvelope` keeps transport-control fields that are genuinely per-write:
`message_id`, `append_enter`, `choice_decider_sessions`, `quiet_window`, and
`quiescence_timeout`. It replaces `rendered` and R1-only UI attribution fields
with a structured message payload containing:

- body text,
- created timestamp,
- namespace,
- canonical sender identity plus optional display name,
- canonical target identity plus optional display name,
- canonical co-recipient identities plus optional display names,
- authenticated identity, when present.

The relay constructs those fields after routing and authorization. Transports
consume them but do not infer or mutate attribution. The namespace is the
routing namespace used to qualify canonical `session@namespace` identities and
metadata emitted for the delivery. The structured payload carries `namespace`
rather than `bundle_name`: `namespace` is the general term for bundle and
relay-wide namespaces (`GLOBAL`, `EXTERNAL`, etc.).

### Decision: `DeliveryEnvelope` owns a transport-level message struct

`DeliveryEnvelope` uses a transport-level message struct rather than embedding
`EnvelopeRenderInput` directly. Coder transports convert that struct into
`EnvelopeRenderInput` when rendering pane-envelope text. This keeps UI delivery
independent from pane-envelope naming while preserving one structured payload
shape across transports. Task 2.2 covers adding the transport-safe
address/attribution structs needed for this boundary.

### Decision: coder transports render pane envelopes internally

Tmux and ACP call the pane-envelope renderer from their own delivery task before
writing to the harness. The renderer remains in `src/envelope.rs`, which is a
transport-safe module: it does not import relay internals. Transport rendering
uses the structured fields to build `EnvelopeRenderInput` and then calls
`render_envelope`.

ACP continues to enforce the token budget after rendering because token estimates
apply to the actual prompt text submitted to the ACP runtime. Tmux continues to
paste rendered envelope text after quiescence.

### Decision: UI reads the same structured payload

`UiTransport` builds `incoming_message` events from the structured message
payload. It does not receive pane-envelope text and does not parse rendered
strings. UI delivery remains success-on-broadcast, with the existing reconnect
wait and delivery outcome phases.

### Decision: relay owns metadata, not representation

The relay remains responsible for target resolution, canonical identities,
display-name lookup, sender authentication, cc derivation, and metadata
inscriptions. It stops deciding how a transport represents that data. The same
structured payload drives both the `relay.send.envelope.metadata` inscription and
transport rendering.

### Decision: `raww` is unchanged

Raw input is not a structured message and does not use pane-envelope rendering.
`raww(content, append_enter)` remains a separate ordered write item and continues
to act as a batch barrier for prior `mailw` items.

## Risks / Trade-offs

- The transport contract type grows because it now carries full message data.
  This is intentional: the data is needed by every representation and was already
  being split between `rendered` text and UI-only fields.
- Rendering moves into multiple transports. Keeping the renderer in one shared
  `src/envelope.rs` function prevents format drift.
- ACP token batching must happen after rendering. Tests should compare rendered
  output and batching behavior before and after the refactor.

## Migration Plan

1. Introduce the structured payload fields while keeping existing behavior.
2. Switch relay worker construction to build structured payloads.
3. Move coder render calls into Tmux and ACP transports.
4. Switch UI to read the structured payload directly.
5. Delete `rendered` and stale R1 comments after all transports compile and tests
   cover the boundary.
