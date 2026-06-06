# Change: Decouple transport layer from relay delivery subsystem

## Why

The relay delivery subsystem is tightly coupled to specific transports. ~2,200
lines of ACP-specific delivery logic (`acp_delivery.rs`, `acp_state.rs`,
`dispatch/worker.rs`, `permission_state.rs`, `observability.rs`) live inside
`src/relay/delivery/`. Tmux pane operations are similarly embedded in
`relay/tmux.rs` and `relay/lifecycle.rs`. Adding or changing a transport
requires editing relay internals. The `Transport` trait and enum dispatch model
give each transport a clean home and let the relay worker dispatch generically.

## What Changes

- Introduce a synchronous `Transport` trait in `src/transports/contract.rs`
  with a `TransportImpl` enum for dispatch (no `Box<dyn>`, no added deps).
- Move all ACP-specific delivery code from `relay/delivery/` into `src/acp/`
  (transport impl, state, permission, observability).
- Move all Tmux-specific delivery code from `relay/` into `src/tmux/` (pane
  operations, session lifecycle primitives, transport impl with quiescence loop).
- Restructure the relay delivery worker to dispatch through `TransportImpl`;
  remove all direct transport imports from `relay/delivery/`.
- Inbound events (ACP replay entries, permission requests) move from
  callbacks + shared state to a transport-owned `mpsc` channel; the worker
  re-subscribes after every `startup()` call. **This is a real restructure in
  Slice 2, not a minor addition.**
- UI transport (`ui_delivery.rs`) is excluded — its stateless registry model
  does not fit the worker lifecycle. Slice 5 is deferred.

## Impact

- Affected specs: `transport-abstraction` (new), `acp-client` (modified)
- Affected code: `src/relay/delivery/`, `src/acp/`, `src/relay/tmux.rs`,
  `src/relay/lifecycle.rs`, new `src/transports/`, new `src/tmux/`
- No user-visible behavior change in Slices 1–4
