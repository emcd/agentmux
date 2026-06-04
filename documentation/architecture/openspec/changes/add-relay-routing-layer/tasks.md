## 1. Resolution stage (operation-agnostic)

- [ ] 1.1 Add a `routing` module with `ResolvedRoute` / `ResolvedTarget`;
  resolve the dispatch bundle plus each target's hosting bundle from the
  principal-id suffix and the catalog; load and memoize peer bundle
  config/authz/runtime once per bundle. No behavior change yet.
- [ ] 1.2 Unit tests: bare target, peer target, unknown bundle, unknown member,
  relay-wide, multi-target fan-out.

## 2. Authorization stage (uniform, profile-driven)

- [ ] 2.1 Add `OperationProfile` (capability + addressing only — no cross-bundle
  policy field); resolve the requester's controls in the dispatch bundle; map
  the requester-to-target relationship to a uniform scope tier (self /
  all:home / all:all) and compare against the requester's configured scope.
- [ ] 2.2 Tests for the uniform threshold (same-bundle self / non-self;
  cross-bundle under home vs all:all; a capability capped below all:all fails
  cross-bundle naturally).

## 3. Migrate operations onto the spine

- [ ] 3.1 Send: route via the shared resolver; tighten cross-bundle delivery to
  require `all:all` (**BREAKING**; aligns code with the existing Send scope
  spec).
- [ ] 3.2 Look: replace `resolve_look_target_bundle` + the `authorize_look`
  cross_bundle flag with its `OperationProfile`.
- [ ] 3.3 List: enable cross-bundle enumeration under `all:all` (fixes the
  requester-in-target-authz defect).
- [ ] 3.4 Raww: no code override — confirm its policy-schema cap (`all:home`)
  makes cross-bundle raww fail the uniform threshold naturally.

## 4. Spec, release notes, decomposition

- [ ] 4.1 Land the `session-relay` deltas (uniform cross-bundle model + Send
  scope reconciliation + cross-bundle list).
- [ ] 4.2 Release notes: Send cross-bundle delivery now requires `all:all`
  (**BREAKING** for permit-all-reliant callers).
- [ ] 4.3 Hand off to `todos/relay/71`: decompose handlers.rs / authorization.rs
  along the resolution / capability-check / policy-loading seams this layer
  creates.
