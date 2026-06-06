# Change: Add relay routing/authz dispatch layer

## Why

Each cross-bundle-capable operation (Send, Look, Raww, List) re-implements
routing and authorization independently inside its handler. They disagree on
which authz context the requester lives in: Send and Look resolve the requester
in their own bundle, List resolves the requester in the target bundle
(`validation_unknown_sender` for foreign requesters), and Raww hard-rejects
cross-bundle entirely. Every new cross-bundle capability requires editing a
handler. Source: `designs/relay/7`.

## What Changes

- Introduce `src/relay/routing.rs` with a shared three-stage dispatch model:
  resolution → authorization → handler body.
- Migrate Send, Look, List, Raww onto the layer in sequence; handler bodies
  receive a `ResolvedRoute` with targets already located and authorized.
- Raww `CrossBundlePolicy` changes from `Forbidden` to `RequireScope(all:all)`;
  policy schema widens to permit `raww = "all:all"`.
- Cross-bundle List is fixed as a side effect: requester controls resolve in
  the dispatch bundle, not the peer bundle.
- Then decompose `handlers.rs` and `authorization.rs` along the clean seams the
  layer creates (`todos/relay/71`).

## Impact

- Affected specs: `relay-routing-layer` (new), `session-relay` (modified)
- Affected code: `src/relay/handlers.rs`, `src/relay/authorization.rs`,
  `src/relay/connection.rs`, new `src/relay/routing.rs`
- **BREAKING** (alpha): cross-bundle Send under `all:home` now returns
  `authorization_forbidden`; was silently permitted despite the existing "Relay
  Send Scope Control" spec already requiring `all:all` — implementation now
  enforces it
- **BREAKING** (alpha): cross-bundle Raww previously returned
  `validation_cross_bundle_unsupported`; now returns `authorization_forbidden`
  when `raww` scope is below `all:all`
