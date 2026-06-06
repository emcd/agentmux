## 1. Relay

- [x] 1.1 Add `bundle_catalog: &BundleCatalog` parameter to `handle_send` in
      `src/relay/handlers.rs`. Update the call site in `src/relay/connection.rs`
      to pass it through.
      NOTE: `handle_send` is reached via `dispatch_request` →
      `handle_request_with_principal` → `handlers::handle_request`, so the
      catalog (and `configuration_root`, needed to load peer-bundle configs/authz
      since the catalog only carries paths) is threaded through that chain. The
      stream call site in `connection.rs` passes the live catalog; the non-stream
      public `handle_request` passes an empty catalog (same-bundle + `@GLOBAL`
      only), keeping its 4-arg signature so test callers are unchanged.
- [x] 1.2 Extend `resolve_explicit_targets` (or replace it with a
      namespace-aware equivalent) to use `split_principal_id` for per-target
      derivation (D1): `@GLOBAL` → relay-wide registry path; `@EXTERNAL` /
      `@RELAY` → `validation_unsupported_namespace`; `@<bundle>` → look up
      bundle in catalog, validate bare session id within that bundle's members;
      bare → sender's bound bundle. Unknown targets across any namespace
      accumulate into `validation_unknown_target` as today.
      NOTE: replaced by `resolve_target_groups`. The relay-wide-sender bare-target
      rejection lives only at the connection layer (`resolve_send_routing_bundle`,
      all-bare case); a bare target reaching `handle_send` resolves in the dispatch
      bundle, because a canonical `<id>@<dispatch-bundle>` target is normalized to
      bare before dispatch and must not be misread as a routing-less bare target.
- [x] 1.3 Group validated targets by namespace and fan out delivery: relay-wide
      targets use the existing `@GLOBAL` delivery path; cross-bundle
      `@<bundle>` targets load the target bundle's configuration from the
      catalog and use it as the delivery context. Delivery groups may be
      dispatched sequentially (D4).
      NOTE: added `AsyncDeliveryTask::sender_bundle_name` so cross-bundle delivery
      attributes/routes the sender to its home bundle (the target's bundle is the
      delivery context); without it the recipient saw the sender canonicalized
      into the target bundle. Per-group permission deciders are loaded from each
      target bundle's authorization context.
- [x] 1.4 Remove `validation_conflicting_namespaces` from the relay error
      contract. Verify no remaining call sites or tests assert it.
      NOTE: removed the `handle_send` guard; no `src/` emit sites remain. The
      former rejection test now asserts fan-out (task 2.4). The live
      `openspec/specs/session-relay/spec.md` still documents the removed
      requirement — left for the coordinator's spec-merge/archive lane.

## 2. Tests

- [x] 2.1 Integration test: bundle-a sends with
      `targets = ["agent@bundle-b", "operator@GLOBAL"]` → message delivered
      to both `agent` in `bundle-b` and `operator@GLOBAL`.
      (`tests/unit/relay_stream/routing.rs::send_fans_out_across_bundle_and_global_namespaces`;
      uses a two-bundle catalog and a coder-less UI member in bundle-b so
      cross-bundle delivery is observable on a registered stream.)
- [x] 2.2 Integration test: bundle-a sends to `"agent@unknown-bundle"` →
      `validation_unknown_target` (unknown bundle treated as unknown target).
      (`tests/unit/relay_stream/routing.rs::send_to_unknown_bundle_target_is_rejected`.)
- [x] 2.3 Integration test: send with an `@EXTERNAL` target →
      `validation_unsupported_namespace`.
      (`tests/unit/relay_stream/routing.rs::send_to_external_namespace_target_is_rejected`.)
- [x] 2.4 Regression: `validation_conflicting_namespaces` no longer returned
      for any combination of `@GLOBAL` and `@bundle` targets.
      (`tests/unit/relay_stream/routing.rs::send_mixing_relay_wide_and_session_targets_fans_out`.)

## 3. Spec / tracking

- [ ] 3.1 Mark `designs/relay/6` nb note as superseded / closed after this
      proposal lands.
      NOTE: deferred until merge lands; flagged for closeout in the relay handoff.
