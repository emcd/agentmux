## 1. Relay

- [ ] 1.1 Add `bundle_catalog: &BundleCatalog` parameter to `handle_send` in
      `src/relay/handlers.rs`. Update the call site in `src/relay/connection.rs`
      to pass it through.
- [ ] 1.2 Extend `resolve_explicit_targets` (or replace it with a
      namespace-aware equivalent) to use `split_principal_id` for per-target
      derivation (D1): `@GLOBAL` → relay-wide registry path; `@EXTERNAL` /
      `@RELAY` → `validation_unsupported_namespace`; `@<bundle>` → look up
      bundle in catalog, validate bare session id within that bundle's members;
      bare → sender's bound bundle. Unknown targets across any namespace
      accumulate into `validation_unknown_target` as today.
- [ ] 1.3 Group validated targets by namespace and fan out delivery: relay-wide
      targets use the existing `@GLOBAL` delivery path; cross-bundle
      `@<bundle>` targets load the target bundle's configuration from the
      catalog and use it as the delivery context. Delivery groups may be
      dispatched sequentially (D4).
- [ ] 1.4 Remove `validation_conflicting_namespaces` from the relay error
      contract. Verify no remaining call sites or tests assert it.

## 2. Tests

- [ ] 2.1 Integration test: bundle-a sends with
      `targets = ["agent@bundle-b", "operator@GLOBAL"]` → message delivered
      to both `agent` in `bundle-b` and `operator@GLOBAL`.
- [ ] 2.2 Integration test: bundle-a sends to `"agent@unknown-bundle"` →
      `validation_unknown_target` (unknown bundle treated as unknown target).
- [ ] 2.3 Integration test: send with an `@EXTERNAL` target →
      `validation_unsupported_namespace`.
- [ ] 2.4 Regression: `validation_conflicting_namespaces` no longer returned
      for any combination of `@GLOBAL` and `@bundle` targets.

## 3. Spec / tracking

- [ ] 3.1 Mark `designs/relay/6` nb note as superseded / closed after this
      proposal lands.
