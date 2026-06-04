# Change: Relay-wide routing and authorization dispatch layer

## Why

Every time cross-bundle support is added to an operation (Send, then Look; List
still pending) we make per-operation changes that re-implement two
operation-agnostic concerns inside each handler: resolving a target's hosting
bundle from its principal-id suffix, and authorizing the requester to reach it.
Worse, the operations disagree on the one decision that matters — *which
authorization context the requester is resolved in*:

- **Send** authorizes the requester in its own (dispatch) bundle, but its gate
  is effectively a no-op: `authorize_send` requires only `self` scope, so any
  configured `send` scope passes and cross-bundle send is permit-all in code —
  even though `Relay Send Scope Control` in the spec already says cross-bundle
  send requires `all:all` and home-only is rejected. The code diverges from its
  own spec.
- **Look** (after `add-cross-bundle-look`) authorizes the requester in its
  dispatch bundle and requires `all:all` to cross the boundary.
- **List** authorizes in the *target* bundle (namespace routing loads the named
  bundle's authorization). A foreign requester is not a member there, so
  `controls_for_requester` returns `validation_unknown_sender` — cross-bundle
  list is blocked by an accident of where authz is loaded, not by policy.

This is an architecture smell. The unifying invariant should be: **the requester
is always authorized in its home/dispatch bundle; a peer bundle supplies only
target existence and runtime/transport context, never the requester's controls.**

## What Changes

- Introduce a routing/authorization dispatch layer between the connection layer
  (`resolve_effective_bundle`) and operation handlers. It produces a resolved,
  already-authorized request so handlers stop parsing principal-id suffixes,
  loading peer bundles from the catalog, and running per-operation authz.
- Three stages: (1) operation-agnostic **resolution** into a `ResolvedRoute`
  (dispatch bundle + resolved targets carrying their bundle / runtime /
  transport), (2) uniform, fully data-driven **authorization**, with the
  requester's controls always resolved in the dispatch bundle, (3) thin
  operation **body**. See design.md.
- **Authorization is entirely data-driven — no operation has a hardcoded
  cross-bundle policy in code.** The layer classifies the requester-to-target
  relationship (self / same-bundle / cross-bundle), maps it to a uniform scope
  tier (`self` / `all:home` / `all:all`), and checks the requester's *configured*
  scope for the operation's capability against it. The relay never decides per
  operation whether crossing is allowed.
  - Look and List capabilities are configurable to `all:all`, so cross-bundle
    works whenever an operator grants it. (List cross-bundle is enabled by this
    change, fixing the requester-in-target-authz defect.)
  - **Send** currently bypasses the threshold (`authorize_send` uses a `SelfOnly`
    minimum that always passes); applying the uniform threshold makes
    cross-bundle send require `all:all`, complying with the existing `Relay Send
    Scope Control` spec. **BREAKING** for callers relying on permit-all
    cross-bundle send under `all:home`.
  - **Raww** stays intra-bundle with no code override: the policy schema caps the
    `raww` control at `all:home`, so `raww = all:all` is not configurable and a
    cross-bundle raww request fails the uniform threshold. Enabling it later is a
    policy-schema change, not a routing-layer change.
- Whether a capability can be configured to cross-bundle (`all:all`) scope lives
  in the policy schema's per-capability allowed-scope set, not in routing code.
- Relay-wide identity admin (`new`/`change`/`introspect`) and the `@GLOBAL`
  registry list are recognized addressing modes of the same dispatch spine
  rather than bypasses.

## Impact

- Affected specs: `session-relay` (uniform cross-bundle authorization model;
  Send scope control reconciliation; cross-bundle list enablement).
- Affected code: `src/relay/connection.rs` (routing seam), `src/relay/handlers.rs`
  and `handlers/*` (handlers slim to bodies), `src/relay/authorization.rs`
  (capability/profile checks split from policy loading and session resolution).
- Sequencing: this layer SHOULD land before `todos/relay/71` (handlers /
  authorization decomposition), so 71 draws module seams around thin operation
  bodies and the layer's stages rather than baking in the per-operation
  duplication. `todos/relay/71` already anticipates this dependency.
- Forward hook: the authorization stage is the single insertion point a future
  target-side ingress filter (ideas/relay/2, cross-relay topology) would use,
  turning that future work into one seam instead of N per-operation edits.

## Non-goals

- Implementing the target-side ingress filter (separate, cross-relay-driven).
- Widening the policy schema to allow `raww = all:all` (a separate policy-schema
  decision; this change neither blocks nor enables it).
- The broader AE parameter/naming audit (todos/mcp/38).
