## 1. Step 0 — Extend routing module with CrossBundlePolicy (no behavior change)

- [ ] 1.1 Extend `src/relay/routing.rs` (already has `OperationProfile`,
      `ResolvedRoute`, `ResolvedTarget`, `authorize_route`); add
      `CrossBundlePolicy { Forbidden, RequireScope(ScopeTier), PermitAll }`
      and wire it into `authorize_route`
- [ ] 1.2 Validate: `cargo check` passes; no behavior change

## 2. Step 1 — Migrate Send onto the layer (no behavior change)

- [ ] 2.1 Implement `MultiTarget` resolution stage for Send in `routing.rs`;
      supersedes per-handler `resolve_send_routing_bundle` /
      `resolve_target_groups`
- [ ] 2.2 Set Send `CrossBundlePolicy = RequireScope(all:all)` (already
      enforced; migrate existing behavior onto the shared table, no change)
- [ ] 2.3 Update `handle_send` to receive `ResolvedRoute`; remove inline
      routing/authz boilerplate
- [ ] 2.4 Validate: `cargo test` passes; no behavior change

## 3. Step 2 — Migrate Look onto the layer (no behavior change)

- [ ] 3.1 Implement `SingleTarget` resolution stage for Look in `routing.rs`;
      supersedes `resolve_look_target_bundle`
- [ ] 3.2 Set Look `CrossBundlePolicy = RequireScope(all:all)` (already
      enforced; migrate existing behavior, no change)
- [ ] 3.3 Update `handle_look` to receive `ResolvedRoute`; remove
      `resolve_look_target_bundle` and `authorize_look`
- [ ] 3.4 Validate: `cargo test` passes; no behavior change

## 4. Step 3 — Migrate List onto the layer (no behavior change)

- [ ] 4.1 Implement `BundleEnumerate` resolution stage for List in `routing.rs`
- [ ] 4.2 Set List `CrossBundlePolicy = RequireScope(all:all)` (requester
      already resolves in home bundle; preserve existing behavior)
- [ ] 4.3 Update `handle_list` to receive `ResolvedRoute`; remove inline
      routing/authz
- [ ] 4.4 Validate: `cargo test` passes; no behavior change

## 5. Step 4 — Migrate Raww; enable cross-bundle under all:all (BREAKING)

- [ ] 5.1 Implement `SingleTarget` resolution stage for Raww in `routing.rs`
- [ ] 5.2 Set Raww `CrossBundlePolicy = RequireScope(all:all)`; replaces flat
      `authorize_raww` → `authorize_scope` path from issues/relay/24
- [ ] 5.3 Widen raww allowed-scope set in `parse_policy_controls` to permit
      `all:all`
- [ ] 5.4 Update `handle_raww` to receive `ResolvedRoute`; remove hard
      `validation_cross_bundle_unsupported` rejection
- [ ] 5.5 Update TUI operator policy to `raww = "all:all"` (required or
      `@GLOBAL` → bundle raww regresses; this is the 0.7.0 TUI blocker fix)
- [ ] 5.6 Update `relay_wide_raww_routes_to_bundle_target_by_suffix` test
      (routing.rs): was passing at `all:home`; must now expect
      `authorization_forbidden` — add a companion test under `all:all` that
      reaches dispatch
- [ ] 5.7 Validate: `cargo test` passes; cross-bundle raww succeeds under
      `all:all`; returns `authorization_forbidden` under `all:home`

## 6. Step 5 — Decompose handlers.rs and authorization.rs (todos/relay/71)

- [ ] 6.1 Split `handlers.rs` along thin operation bodies (routing/authz
      boilerplate now absent; submodule seams are clean)
- [ ] 6.2 Split `authorization.rs` along (policy loading | capability/profile
      checks | session resolution) seams
- [ ] 6.3 Validate: `cargo test` passes; no behavior change
