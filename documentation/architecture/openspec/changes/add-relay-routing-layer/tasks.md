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

> **Landed first** as the fast-tracked 0.7.0 blocker fix. This slice does the
> authz wiring only: `handle_raww` builds a single-target `ResolvedRoute` inline
> and authorizes via the existing `authorize_route` / `required_tier`
> (data-driven by the policy schema's allowed-scope set, per the `routing.rs`
> module contract), adding a `Capability::Raww` variant. The flat
> `authorize_raww` → `authorize_scope` path is deleted. The Step 0
> `CrossBundlePolicy` enum and the dedicated `SingleTarget` resolution stage
> (5.1) are deferred to the later, no-behavior-change migration slices — they are
> not needed for the blocker.

- [ ] 5.1 Implement `SingleTarget` resolution stage for Raww in `routing.rs`
      (deferred — handle_raww still locates its own target this slice)
- [x] 5.2 Raww requires `all:all` for cross-namespace reach via
      `required_tier` (replaces the flat `authorize_raww` → `authorize_scope`
      path from issues/relay/24, now deleted). The `CrossBundlePolicy` enum form
      is deferred to Step 0/later slices; behavior is already correct.
- [x] 5.3 Widen raww allowed-scope set in `parse_policy_controls` to permit
      `all:all`
- [x] 5.4 `handle_raww` builds and authorizes a `ResolvedRoute` through the
      spine (there was no `validation_cross_bundle_unsupported` rejection in code
      to remove). Receiving a pre-built route from a resolution stage is the
      deferred 5.1 part.
- [x] 5.5 Update TUI operator policy to `raww = "all:all"` (required or
      `@GLOBAL` → bundle raww regresses; this is the 0.7.0 TUI blocker fix)
- [x] 5.6 Update `relay_wide_raww_routes_to_bundle_target_by_suffix` test
      (routing.rs): now requires `all:all` and reaches dispatch; added
      `relay_wide_raww_into_bundle_denied_under_home_scope` (forbidden under
      `all:home`) and `same_bundle_raww_permitted_under_home_scope`
- [x] 5.7 Validate: `cargo test` passes (250); cross-bundle raww succeeds under
      `all:all`; returns `authorization_forbidden` under `all:home`

## 6. Step 5 — Decompose handlers.rs and authorization.rs (todos/relay/71)

- [ ] 6.1 Split `handlers.rs` along thin operation bodies (routing/authz
      boilerplate now absent; submodule seams are clean)
- [ ] 6.2 Split `authorization.rs` along (policy loading | capability/profile
      checks | session resolution) seams
- [ ] 6.3 Validate: `cargo test` passes; no behavior change
