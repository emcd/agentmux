## 1. Resolution stage (operation-agnostic)

- [x] 1.1 Add a `routing` module with `ResolvedRoute` / `ResolvedTarget`;
  resolve the dispatch bundle plus each target's hosting bundle from the
  principal-id suffix and the catalog; load and memoize peer bundle
  config/authz/runtime once per bundle. No behavior change yet.
- [x] 1.2 Unit tests: bare target, peer target, unknown bundle, unknown member,
  relay-wide, multi-target fan-out. (Covered across the `relay_stream` routing,
  list, and look suites: bare/relay-wide/fan-out in `routing.rs`, peer/unknown
  member in `list.rs` + `look.rs`, unknown bundle in `list.rs`/`look.rs`/`routing.rs`.)

## 2. Authorization stage (uniform, profile-driven)

- [x] 2.1 Add `OperationProfile` (capability + addressing only — no cross-bundle
  policy field); resolve the requester's controls in the dispatch bundle; map
  the requester-to-target relationship to a uniform scope tier (self /
  all:home / all:all) and compare against the requester's configured scope.
- [x] 2.2 Tests for the uniform threshold (same-bundle self / non-self;
  cross-bundle under home vs all:all; a capability capped below all:all fails
  cross-bundle naturally — `raww` stays intra-bundle, rejected before the
  threshold; the unit `relay.rs` look self / same-bundle-non-self cases cover
  the self vs home split).

## 3. Migrate operations onto the spine

- [x] 3.1 Send: route via the shared resolver; tighten cross-bundle delivery to
  require `all:all` (**BREAKING**; aligns code with the existing Send scope
  spec).
- [x] 3.2 Look: replace `resolve_look_target_bundle` + the `authorize_look`
  cross_bundle flag with its `OperationProfile`. (`resolve_look_target_bundle`
  still resolves the peer bundle's runtime context; the cross_bundle authz flag
  is gone, folded into the uniform tier.)
- [x] 3.3 List: enable cross-bundle enumeration under `all:all` (fixes the
  requester-in-target-authz defect via `handle_list_routed` + the connection-
  layer dispatch/enumerate split).
- [x] 3.4 Raww: no code override — confirmed its policy-schema cap (`all:home`)
  makes cross-bundle raww fail the uniform threshold naturally; it also still
  rejects cross-bundle targets up front (`validation_cross_bundle_unsupported`).

## 4. Spec, release notes, decomposition

- [x] 4.1 Land the `session-relay` deltas (uniform cross-bundle model + Send
  scope reconciliation + cross-bundle list).
- [x] 4.2 Release notes: Send cross-bundle delivery now requires `all:all`
  (**BREAKING** for permit-all-reliant callers). Recorded in the implementation
  commit message (no `CHANGELOG` file in this repo).
- [x] 4.3 Hand off to `todos/relay/71`: decompose handlers.rs / authorization.rs
  along the resolution / capability-check / policy-loading seams this layer
  creates. (The seams now exist — `routing.rs`, `authorize_route`,
  `handle_list_routed`; the decomposition itself remains `todos/relay/71`.)
