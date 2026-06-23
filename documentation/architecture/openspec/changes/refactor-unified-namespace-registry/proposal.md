# Change: Refactor live sessions into a unified namespace registry

## Why

The relay now routes by namespace-qualified principal IDs, but the live stream
registry still splits bundle sessions from relay-wide/UI sessions. That split
preserves `RegistryKey::RelayWide`, a special `GLOBAL` list path, and duplicated
target lookup/capability logic even though `principal_id` already carries the
namespace in `session@namespace` form.

## What Changes

- Replace the split stream registry key with a single session registry keyed by
  canonical `principal_id`, populated by both bundle runtime startup and stream
  hello registration.
- Store namespace, bare session id, registration source, transport binding, and
  transport capability flags on each registry entry.
- Remove `RegistryKey::RelayWide` and the relay-wide lookup/listing special path;
  `GLOBAL` is a namespace in the same registry model as bundle namespaces.
- Resolve target existence, delivery binding, and target-side transport
  capabilities from registry entries through one helper instead of bifurcating
  bundle-vs-relay-wide logic.
- Preserve namespace-centric routing and authorization: principal-id suffixes
  still classify target namespace, and policy checks still run in the requester
  home namespace.

## Impact

- Affected specs: `session-relay`, `relay-routing-layer`
- Related notebook items: `ideas/relay/5`; companion capability-attribute
  design from `ideas/relay/6` is represented here as per-entry capability flags.
- Downstream requirements that mention legacy relay-wide/bundle registry paths
  are deferred unless they are directly modified by this proposal's deltas.
- Affected code:
  - `src/relay/stream.rs` — registry key, entry shape, registration, lookup,
    eviction, listing, and event fan-out helpers
  - `src/relay/connection.rs` — hello registration and connection binding
    against the canonical-principal registry model
  - `src/relay/delivery/async_worker.rs`, `dispatch/worker.rs`, and
    `dispatch/payload.rs` — consume registry-provided transport/runtime binding
    for coder and stream-delivered targets
  - `src/relay/handlers/dispatch.rs` — remove the dedicated `GLOBAL` list bypass
  - `src/relay/handlers/routed.rs`, `look.rs`, `raww.rs`, and `send.rs` — use
    unified target-entry resolution for existence, capabilities, and delivery
    binding
  - `src/relay/routing.rs` — retire relay-wide target flags that exist only to
    select a separate registry path

Bundle-subject operations (`up`, `down`, choice decisions, and bundle-scoped
request dispatch) continue to resolve configured bundles through the existing
namespace routing path; this change targets session/target registry lookup, not
bundle lifecycle command routing.

No relay wire protocol change is intended. Canonical session IDs,
authorization scope semantics, request `namespace` routing, and list/send/look/
raww response shapes remain unchanged. `validation_unsupported_operation` is the
existing observable error for look/raww targets whose transport capability flags
do not support the requested operation.
