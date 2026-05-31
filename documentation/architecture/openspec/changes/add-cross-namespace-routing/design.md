## Context

`add-global-namespace-routing` wires `@GLOBAL` targets through the relay-wide
registry and deliberately rejects mixed-namespace sends with
`validation_conflicting_namespaces`. That error code was always a placeholder:
the spec says "Cross-namespace fan-out in one request is not supported in this
slice." This proposal delivers that fan-out.

The existing infrastructure is almost sufficient. `split_principal_id` already
parses `@<namespace>` suffixes. `RegistryKey::Session { bundle_name, session_id }`
already exists for bundle-scoped targets. `BundleCatalog` is already threaded
through the connection layer and accessible to `handle_send`'s call site.
The missing pieces are: (1) per-target namespace derivation in
`resolve_explicit_targets`, and (2) cross-bundle delivery dispatch.

## Goals / Non-Goals

Goals:
- A single `Send` may list targets from any combination of known namespaces
  (`@GLOBAL`, `@bundle-a`, `@bundle-b`, bare).
- Each target is resolved against its namespace-specific registry.
- Delivery fans out independently per namespace group; per-target results are
  returned in `RelayResponse::Send` as today.
- `validation_conflicting_namespaces` is retired.

Non-Goals:
- `@EXTERNAL` / `@RELAY` routing (reserved; rejected this slice).
- Cross-namespace broadcast (`broadcast = true` stays bundle-scoped).
- Fine-grained cross-bundle send authorization (deferred; permit-all for
  authenticated sessions this slice).
- `Look` / `Raww` cross-namespace (relay-wide UI sessions are not
  tmux-pane-backed; cross-bundle Look is a separate concern).

## Decisions

**D1 — Per-target namespace derivation via `split_principal_id`.**
Each target is split on `@` via the existing `split_principal_id` helper.
The namespace suffix identifies the registry:
- `GLOBAL` → `RegistryKey::RelayWide` (add-global-namespace-routing path)
- `EXTERNAL` / `RELAY` → `validation_unsupported_namespace`
- `<bundle-name>` (any other non-empty namespace) → `RegistryKey::Session {
  bundle_name: <namespace>, session_id: <bare_id> }`
- bare (no `@` suffix) → sender's bound bundle; relay-wide sender with bare
  target → `validation_missing_routing_namespace`

Note: `<bundle-name>` targets are already the "Session" branch of
`classify_principal_id`. The only change from the current `registry_key_for_target`
behaviour is that the bundle_name is taken from the target suffix, not from
the sender's bound bundle.

**D2 — Validate all targets before any delivery (batch validation).**
The relay validates every resolved target (namespace known, session registered
or configured) before attempting any delivery. If any target is unknown, the
whole request fails with `validation_unknown_target`. This is consistent with
current same-bundle behavior and avoids partial-delivery state that would
require new response semantics.

**D3 — `handle_send` receives `bundle_catalog`.**
Cross-bundle targets require the target bundle's `BundleRuntimePaths` (to load
`BundleConfiguration` for member validation and transport resolution). The
`BundleCatalog` (`Arc<HashMap<String, BundleRuntimePaths>>`) is already
available at the call site in `connection.rs`; the change is to thread it into
`handle_send`. The sender's `BundleConfiguration` is still needed for sender
identity resolution, authorization, and bare-target expansion.

**D4 — Delivery grouped by namespace segment.**
After validation, resolved targets are grouped by (namespace, bundle_config).
Delivery for each group proceeds using that namespace's bundle context and
registry. Groups are dispatched sequentially; concurrency within a group
follows existing delivery semantics. Per-target results in `RelayResponse::Send`
remain the observable contract.

**D5 — Authorization: permit-all for cross-bundle sends (this slice).**
Any authenticated session may send to any `@<bundle>` target without additional
authorization checks. The existing `all` vs `home` send-scope vocabulary is the
right hook for this: a `home`-scoped sender should be restricted to its bound
namespace; an `all`-scoped sender may send cross-bundle. Wiring this up is
tracked in `todos/relay/62` and deferred to keep this slice focused.

**D6 — `broadcast = true` remains bundle-scoped.**
Expanding "broadcast" across all bundles requires enumerating the full catalog
at request time, which risks unbounded fan-out and has no clear semantics for
relay-wide principals. Restricted to the sender's bound bundle for now; a
separate proposal can define cross-namespace broadcast if needed.

## Risks / Trade-offs

- **Batch validation fail-fast hides partial reachability**: if `agent@bundle-b`
  is unknown but `agent@bundle-a` is known, the whole request fails. Callers
  can split sends to handle partial reachability themselves.
- **Sequential namespace-group dispatch adds latency for large fan-out**: for
  typical coordination payloads (2–4 targets) this is negligible.
- **Cross-bundle delivery bypasses target-bundle authorization**: a bundle-a
  session can send to any bundle-b member that is registered. Acceptable for
  alpha single-operator deployments.
