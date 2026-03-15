# Change: Add relay stream transport with hello registration

## Why

Current relay IPC is request/response over short-lived socket connections.
TUI history/status and future richer coordination flows benefit from one
persistent full-duplex channel with explicit client registration and pushed
updates.

## What Changes

- Define persistent full-duplex relay client transport over Unix socket.
- Define mandatory `hello` registration frame for stream sessions.
- Keep static bundle recipients routable independent of `hello`.
- Define endpoint classes (`agent`, `ui`) and routing behavior:
  - static configured recipients use agent routing semantics,
  - dynamically registered `ui` recipients use stream event delivery semantics.
- Define relay push event contract for inbound message and terminal delivery
  updates.
- Define reconnect/replace semantics for same identity stream sessions.
- Define disconnected UI recipient queue behavior using existing relay async
  delivery queue model (no competing queue mechanism).

## Impact

- Affected specs:
  - `session-relay`
  - `runtime-bootstrap`
- Affected code (implementation follow-up, not in this proposal):
  - relay socket accept loop and connection lifecycle management
  - relay frame protocol and stream writer/reader state
  - MCP relay client transport (agent class hello)
  - TUI relay client transport (ui class hello)
