## 1. Step 0 — Routing module scaffolding (no behavior change)

- [ ] 1.1 Create `src/relay/routing.rs`; define `ResolvedTarget`,
      `ResolvedRoute`, `OperationProfile`, `CrossBundlePolicy` types
- [ ] 1.2 Register `mod routing;` in `src/relay/mod.rs`
- [ ] 1.3 Validate: `cargo check` passes; no behavior change

## 2. Step 1 — Migrate Send onto the layer

- [ ] 2.1 Implement `MultiTarget` resolution stage for Send in `routing.rs`;
      supersedes `resolve_send_routing_bundle` and `resolve_target_groups`
- [ ] 2.2 Set Send `CrossBundlePolicy = RequireScope(all:all)`; resolve
      requester controls from dispatch bundle (aligns implementation with
      existing "Relay Send Scope Control" spec — was unenforced `PermitAll`)
- [ ] 2.3 Update `handle_send` to receive `ResolvedRoute`; remove inline
      routing/authz boilerplate
- [ ] 2.4 Update tests: cross-bundle Send under `all:home` now returns
      `authorization_forbidden`; add test asserting it; update any tests that
      assumed silent cross-bundle permit
- [ ] 2.5 Validate: `cargo test` passes

## 3. Step 2 — Migrate Look onto the layer

- [ ] 3.1 Implement `SingleTarget` resolution stage for Look in `routing.rs`;
      supersedes `resolve_look_target_bundle`
- [ ] 3.2 Set Look `CrossBundlePolicy = RequireScope(all:all)`
- [ ] 3.3 Update `handle_look` to receive `ResolvedRoute`; remove
      `resolve_look_target_bundle` and `authorize_look`
- [ ] 3.4 Validate: `cargo test` passes; cross-bundle look behavior unchanged

## 4. Step 3 — Migrate List; fix requester-in-target-authz bug

- [ ] 4.1 Implement `BundleEnumerate` resolution stage for List in `routing.rs`
- [ ] 4.2 Set List `CrossBundlePolicy = RequireScope(all:all)`; resolve
      requester controls from dispatch bundle (not peer bundle)
- [ ] 4.3 Update `handle_list` to receive `ResolvedRoute`; remove inline
      routing/authz
- [ ] 4.4 Validate: `cargo test` passes; cross-bundle list resolves requester
      in home bundle; `validation_unknown_sender` no longer returned for
      foreign requesters with sufficient scope

## 5. Step 4 — Migrate Raww; enable cross-bundle under all:all

- [ ] 5.1 Implement `SingleTarget` resolution stage for Raww in `routing.rs`
- [ ] 5.2 Set Raww `CrossBundlePolicy = RequireScope(all:all)`
- [ ] 5.3 Widen raww allowed-scope set in `parse_policy_controls` to permit
      `all:all`
- [ ] 5.4 Update `handle_raww` to receive `ResolvedRoute`; remove hard
      `validation_cross_bundle_unsupported` rejection
- [ ] 5.5 Validate: `cargo test` passes; cross-bundle raww succeeds under
      `all:all`; returns `authorization_forbidden` under `all:home`

## 6. Step 5 — Decompose handlers.rs and authorization.rs (todos/relay/71)

- [ ] 6.1 Split `handlers.rs` along thin operation bodies (routing/authz
      boilerplate now absent; submodule seams are clean)
- [ ] 6.2 Split `authorization.rs` along (policy loading | capability/profile
      checks | session resolution) seams
- [ ] 6.3 Validate: `cargo test` passes; no behavior change
