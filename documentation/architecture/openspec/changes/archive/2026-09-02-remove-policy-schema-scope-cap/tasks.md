## 1. Sync the requirement deltas

- [x] 1.1 Sync `authorization-scope` — `Uniform Cross-Bundle Authorization
      Model` no longer conditions cross-bundle reach on a policy-schema cap,
      and no longer carries the `Capability not configurable to cross-bundle
      scope fails uniformly` scenario
- [x] 1.2 Sync `relay-routing-layer` — `Authorization Stage` keeps the rule
      that no per-operation cross-namespace logic lives in handler or routing
      code, without naming a schema allowed-scope set as its authority

## 2. Direct edits (no delta mechanism governs these)

- [x] 2.1 `specs/relay-routing-layer/spec.md` Purpose preamble: remove the
      parenthetical naming the policy schema's per-capability allowed-scope set
      as the sole authority for cross-bundle reach
- [x] 2.2 `src/relay/routing.rs` module documentation: remove the sentence
      asserting that cross-bundle reach is governed by a per-capability
      allowed-scope set, keeping the surrounding description of uniform tier
      classification

## 3. Verify

- [x] 3.1 `scripts/verify-openspec-deltas.py remove-policy-schema-scope-cap`
      reports zero errors and exactly one dropped scenario, that being the
      unsatisfiable one this change retires; re-run immediately before sync in
      case a live spec moved underneath a delta
- [x] 3.2 `openspec validate --all --strict` passes
- [x] 3.3 Confirm no spec or source comment still describes a per-capability
      allowed-scope cap, a policy-schema cap, or a capability being unreachable
      cross-bundle until a schema is widened
- [x] 3.4 Confirm the guard comment at `src/relay/authorization/loading.rs`
      still stands and now agrees with every specification statement about
      control scopes
- [x] 3.5 Confirm no remaining scenario has a precondition that no
      configuration can establish

## 4. Close out

- [x] 4.1 Mark `todos/openspec/4` complete
- [x] 4.2 Confirm `todos/openspec/5` (distribute verb authorizability) is
      unblocked, which was the reason this change had to land first
