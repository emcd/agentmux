# Change: Add cross-namespace fan-out routing for Send

## Why

`add-global-namespace-routing` implements `@GLOBAL` delivery via suffix
inference but blocks mixed-namespace sends with `validation_conflicting_namespaces`
as an explicit temporary restriction. The next step is to lift that restriction:
a single `Send` should fan out across `@GLOBAL`, `@bundle-a`, and `@bundle-b`
targets in one request. This closes the coordination gap where a bundle-bound
agent cannot CC the operator alongside peers in other bundles in one envelope.

## What Changes

- **Cross-bundle `@<bundle>` routing in `handle_send`**: targets with a
  `@<bundle>` suffix are resolved against that bundle's registry; the relay
  fans out delivery independently per namespace group.
- **Retire `validation_conflicting_namespaces`**: mixed-namespace sends no
  longer return this error; all valid targets across namespaces are delivered.
- **`@EXTERNAL` / `@RELAY` targets rejected**: reserved for future cross-relay
  routing; the relay returns `validation_unsupported_namespace`. Keeps this
  error code in the contract.
- **Broadcast remains bundle-scoped**: `broadcast = true` expands targets
  within the sender's bound bundle only; cross-namespace broadcast semantics
  are deferred.

## Impact

- Affected specs: `session-relay`
- Affected code: `src/relay/handlers.rs`, `src/relay/connection.rs`
- Retires: `validation_conflicting_namespaces`
- Depends on: `add-global-namespace-routing` (merged and archived)
- Closes: `designs/relay/6`
