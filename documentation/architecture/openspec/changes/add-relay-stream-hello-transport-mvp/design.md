## Context

The relay currently accepts one request per connection and returns one response.
For low-latency UI updates and consistent client lifecycle semantics, we need a
single long-lived transport model shared by MCP and TUI clients.

## Goals

- Establish one persistent relay stream model for clients.
- Require explicit client registration with `hello`.
- Preserve static recipient routability independent of client registration.
- Reuse existing relay queue/delivery machinery rather than adding competing
  queue mechanisms.

## Non-Goals

- Cross-bundle transport in MVP.
- Relay callback sockets to client-provided return addresses.
- Durable event/history replay store in MVP.

## Decisions

- Decision: clients use long-lived full-duplex relay connections.
  - requests and responses are framed on same connection,
  - relay may push async events on same connection.

- Decision: each stream starts with required `hello` frame containing:
  - `client_class` (`agent` | `ui`)
  - requester identity/session
  - associated bundle context
  - protocol schema version.

- Decision: static configured recipients remain routable without requiring an
  active stream or prior hello from that recipient.

- Decision: hello registration binds live endpoint behavior and readiness:
  - agent class clients represent relay-connected agent endpoints,
  - ui class clients represent relay-connected UI endpoints.

- Decision: routing uses endpoint class semantics:
  - agent recipients continue prompt-injection/quiescence delivery path,
  - ui recipients receive pushed relay stream events,
  - if ui recipient stream is disconnected, relay keeps pending delivery queued
    and attempts delivery when same identity reconnects.

- Decision: same identity reconnect replaces prior live stream session.

## Risks / Trade-offs

- Trade-off: connection lifecycle complexity increases versus one-shot RPC, but
  enables consistent low-latency updates and removes polling overhead.
- Risk: stream backpressure for slow clients.
  Mitigation: bounded per-connection write queues + fail-fast disconnect policy.
- Risk: reconnect race for identity replacement.
  Mitigation: deterministic "latest hello wins" binding rule.

## Migration Plan

1. Land stream/hello transport contract in OpenSpec.
2. Implement relay framing and connection lifecycle.
3. Migrate MCP client transport to persistent agent-class stream.
4. Migrate TUI client transport to persistent ui-class stream.
