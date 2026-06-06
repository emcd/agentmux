# Change: Add relay routing/authz dispatch layer

## Why

Each cross-bundle-capable operation (Send, Look, Raww, List) resolves routing
and authorization per-handler rather than through a shared spine. Raww
hard-rejects cross-bundle entirely; its flat `authorize_scope` path
(issues/relay/24) under-enforces relative to the Uniform Cross-Bundle
Authorization Model. Every new cross-bundle capability requires editing a
handler. Source: `designs/relay/7`.

## What Changes

- Extend `src/relay/routing.rs` (module already exists) with a `CrossBundlePolicy`
  table and a shared resolution stage; migrate Send, Look, List, Raww handler
  bodies to receive a `ResolvedRoute` with targets already located and authorized.
- Raww `CrossBundlePolicy` changes from `Forbidden` to `RequireScope(all:all)`;
  policy schema widens to permit `raww = "all:all"`. This supersedes the flat
  `authorize_scope` path added in issues/relay/24, raising the required tier for
  `@GLOBAL` → bundle raww from `all:home` to `all:all`. (`all:home` for a
  `@GLOBAL` user covers only `GLOBAL` targets — UI sessions that cannot accept
  raww — making `all:all` the only meaningful tier for cross-bundle raww.)
- TUI operator policy SHALL be updated to `raww = "all:all"` in the same change.
- Send, Look, and List are no-behavior-change migrations onto the shared layer.
- Then decompose `handlers.rs` and `authorization.rs` along the clean seams the
  layer creates (`todos/relay/71`).

## Impact

- Affected specs: `relay-routing-layer` (new), `session-relay` (modified)
- Affected code: `src/relay/handlers.rs`, `src/relay/authorization.rs`,
  `src/relay/connection.rs`, `src/relay/routing.rs` (extended), TUI operator
  policy configuration
- **BREAKING** (alpha): `@GLOBAL` → bundle raww now requires `raww = "all:all"`;
  issues/relay/24 permitted it at `all:home` via a flat scope check. Update TUI
  operator policy in the same slice.
- **BREAKING** (alpha): cross-bundle Raww previously returned
  `validation_cross_bundle_unsupported`; now returns `authorization_forbidden`
  when `raww` scope is below `all:all`
- Send/Look/List: no behavior change — migrations only
