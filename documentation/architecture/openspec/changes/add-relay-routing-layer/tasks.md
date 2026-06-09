## 1. Step 0 — Reconcile spec/design to the data-driven spine (no new enum)

> **Revised (Option B).** The `CrossBundlePolicy` enum is stale: the shipped
> `src/relay/routing.rs` (`OperationProfile` + `required_tier` + the policy
> schema's per-capability allowed-scope set) already is the single authority for
> cross-bundle reach, with no per-operation hardcoded policy. The Raww slice
> proved a new cross-bundle capability is a `Capability` variant plus a schema
> allowed-scope widening, not a handler edit. Step 0 is doc/spec reconciliation
> only.

- [x] 1.1 Rewrite the `relay-routing-layer` spec Authorization Stage requirement
      to describe the existing data-driven mechanism (drop the `CrossBundlePolicy`
      table and the "single reviewable authority" enum language); fix the stale
      `PermitAll` Non-Goal and §2 in `design.md`
- [x] 1.2 Validate: `openspec validate add-relay-routing-layer --strict` passes

## 2. Step 1 — Require qualified targets, then migrate Send (BREAKING)

> **Root cause first.** The relay accepted bare targets and resolved them against
> bundle members + the UI registry, coupling routing to configuration and letting
> a bare id silently resolve to a `@GLOBAL` operator. We require fully-qualified
> targets at the relay and move the bare-id convenience client-side, which makes
> the resolution stage config-free. Only then does the Send migration land on a
> genuinely thin layer.

### 2A. Relay: require fully-qualified targets

- [x] 2.1 Reject bare (unqualified) targets on Send/Look/Raww with
      `validation_unqualified_target`; remove bare→bound-bundle and bare→UI
      resolution from `resolve_target_groups` and the target half of
      `normalize_request_identities`
- [x] 2.2 Implement the config-free `MultiTarget` resolution stage in
      `routing.rs`: classify each target from its suffix alone into
      `ResolvedTarget { bundle_name, session_id, relay_wide }`; supersedes
      `resolve_send_routing_bundle` and the classification half of
      `resolve_target_groups`

### 2B. Clients: fill in the namespace (cross-lane — MCP / TUI lanes)

- [x] 2.3 MCP server fills the caller's bound bundle into a target left
      unqualified before sending (`mcp-tool-surface`); MCP lane
- [x] 2.4 TUI (global user) always qualifies targets (`tui-surface`); TUI lane
- [x] 2.5 Coordinator sequences rollout: client fill-in lands with or before the
      relay's bare-target rejection

### 2C. Namespace-centric dispatch (no borrowed bundle)

> A sender has exactly one home namespace (its bundle, or `GLOBAL`); its controls
> come from that namespace's policy. Stop borrowing a peer bundle for a `GLOBAL`
> sender.

- [x] 2.6 Build a `GLOBAL`/operator authorization context (operator policy +
      relay-wide permission config, no bundle members) so a relay-wide sender is
      authorized without loading a borrowed bundle; decouple
      `load_authorization_context` / `dispatch_request` from a single
      `BundleConfiguration` for relay-wide senders
- [x] 2.7 Drop `resolve_send_routing_bundle`; route Send per-target by namespace;
      a relay-wide `Send` to `@GLOBAL`-only targets succeeds (no
      `validation_missing_routing_namespace`)
- [x] 2.8 Rename `ResolvedRoute.dispatch_bundle_name` → `dispatch_namespace` (and
      `requester_home_namespace` call sites) to match the namespace model

### 2D. Migrate Send onto the config-free layer

- [x] 2.9 Update `handle_send` to obtain its `ResolvedRoute` from the resolution
      stage; keep existence validation (before authz) and delivery assembly
      (after authz) in the body; remove inline suffix parsing / route-building
- [x] 2.10 Validate: `cargo test` passes; no behavior change beyond bare-target
      rejection and the relay-wide `GLOBAL`-only send now succeeding

## 3. Step 2 — Migrate Look onto the layer (no behavior change)

- [x] 3.1 Implement `SingleTarget` config-free resolution stage for Look in
      `routing.rs`; supersedes `resolve_look_target_bundle`
- [x] 3.2 No profile change needed — Look already reaches `all:all` via the
      policy schema allowed-scope set (`required_tier` is data-driven; no
      per-operation enum)
- [x] 3.3 Update `handle_look` to receive `ResolvedRoute`; remove
      `resolve_look_target_bundle` and `authorize_look`
- [x] 3.4 Validate: `cargo test` passes; no behavior change

## 4. Step 3 — Migrate List onto the layer (no behavior change)

- [ ] 4.1 Implement `BundleEnumerate` resolution stage for List in `routing.rs`
- [ ] 4.2 No profile change needed — List already reaches `all:all` via the
      policy schema allowed-scope set; requester resolves in its home bundle
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
