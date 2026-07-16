# Change: Add Embeddable Relay Runtime API

## Why

Agentmux relay semantics are currently reached primarily through the standalone
relay socket and surface-specific clients. Application hosts such as `litrpg`
need the same relay semantics as an in-process Rust API without reimplementing
private socket frames or bypassing relay identity and authorization.

The standalone relay binary must also consume this API. It is the first host of
the embeddable runtime, not a special implementation path beside it.

## What Changes

- Define public relay dispatch, provisioning, and introspection functions as the
  Rust embedding boundary.
- Require embeddable runtime initialization to accept caller-configured config
  and state roots without assuming fixed parent layout or standalone-only root
  behavior.
- Require the standalone relay server, socket protocol, MCP, CLI, stdio, and
  future transports to call the same public relay handlers.
- Require in-process embedding to call public handlers directly, without an
  in-process transport adapter or private Hello/request/response frame loop.
- Add `Content-Type` envelope discrimination for `text/plain`, structured
  Agentmux events, and future extension envelopes.
- Require topology-independent relay semantics across embedded, standalone, and
  sidecar deployments.
- Separate caller-supplied identity descriptors from relay-verified principal
  context in public API types.
- Require `direct_psk` as a public in-memory credential source for embedded
  hosts and dynamic agents.
- Define explicit `accept_ack` timeout and stale-correlation cleanup behavior
  for content types that require an accept ACK.
- Define a transport-neutral delivery execution and observation seam so
  embedders can control runtime lifecycle and deterministic harnesses can drive
  delivery outcomes without exposing worker-registry internals.

## Non-Goals

- No extension registry, versioned extension schema discovery, MCP extension
  polling, or extension submit surface. Those belong to Proposal B.
- No application-domain authorization requirements. Applications such as
  `litrpg` remain responsible for game-domain rules and state mutation.
- No cross-relay trust, discovery documents, `[[trusted-relays]]`, or signed
  assertion topology.
- No public Rust API exposure for private socket Hello/request/response wire
  frame structs.
- No public exposure of invariant-sensitive delivery internals such as worker
  registry entries, internal delivery-task fields, worker-close functions, or
  terminal-outcome completion functions.

## Impact

- Affected specs:
  - `runtime-api`
- Affected code:
  - relay public dispatch and handler modules
  - standalone relay host startup and socket connection handling
  - MCP, CLI, and stdio relay client/adapter paths
  - relay identity verification/provisioning call boundaries
  - envelope model and ACK correlation handling
  - delivery execution, observation, and runtime shutdown boundaries
- Source design notes:
  - `designs/relay-api/1` is the embedding-boundary source of truth
  - `designs/api/2` supplies public dispatch, transport parity, Content-Type,
    and ACK timeout decisions
  - `designs/api/4` supplies the OpenSpec promotion checklist
