# Design: Relay routing/authorization dispatch layer

## Context

Promoted from notebook `designs/relay/7` (Coordinator-approved in substance).
Builds on `add-cross-namespace-routing` (suffix-based Send routing) and
`add-cross-bundle-look`. The root-cause diagnosis: routing and authorization are
entangled with each operation handler, and the handlers disagree on which
authorization context the requester lives in (see proposal.md).

## Invariant

The requester is always authorized in its home/dispatch bundle. A peer bundle
supplies only target existence and runtime/transport context — never the
requester's policy controls.

## Three-stage spine

1. **Resolution (operation-agnostic).** Input: verified principal + request +
   catalog + config. Output: `ResolvedRoute { dispatch_bundle, targets:
   Vec<ResolvedTarget> }` where `ResolvedTarget { bundle_name, runtime_directory,
   session_id, transport, registry_kind }`. All principal-id suffix grammar and
   catalog lookups live here once; peer bundle config/authz/runtime are loaded
   and memoized per bundle. This generalizes Send's existing
   `DeliveryGroup`/`ResolvedTarget` and Look's `resolve_look_target_bundle`.

2. **Authorization (uniform, fully data-driven).** The requester's controls are
   resolved in `dispatch_bundle`. The stage classifies the requester-to-target
   relationship and maps it to a required scope tier on the existing scope
   ladder — uniformly, for every operation:

   - self target → `self`
   - same-bundle non-self target → `all:home`
   - cross-bundle target → `all:all`

   It then checks whether the requester's *configured* scope for the operation's
   capability meets that tier. The only per-operation input is which capability
   (policy control) to read:

   ```
   struct OperationProfile {
       capability: Capability,   // which policies.toml control: look | send | raww | list ...
       addressing: Addressing,
   }
   enum Addressing { SingleTarget, MultiTarget, BundleEnumerate, BundleLevel, RelayWide }
   ```

   There is **no per-operation cross-bundle policy in code**. The relay never
   decides "operation X may/may not cross bundles" — it only compares the
   requester's configured scope against the uniform threshold. Whether a
   capability can ever be configured to a cross-bundle (`all:all`) scope is
   governed by the policy schema's per-capability allowed-scope set
   (`parse_policy_controls`): a policies.toml / policy-schema concern, not
   routing code.

3. **Operation body (handler).** Receives a `ResolvedRoute` with targets located
   and cleared. Implements only operation-specific work: capture snapshot /
   enqueue delivery / write raw / enumerate. No suffix parsing, no catalog
   lookup, no authz.

## Addressing modes (which operations have genuine per-op semantics)

- **SingleTarget** (Look, Raww): one resolved target. Largest immediate win —
  enabling cross-bundle becomes a profile flag.
- **MultiTarget fan-out** (Send): N targets across N bundles. Routing/authz
  factor out; the genuinely op-specific work (per-group permission deciders,
  transport/timeout cross-validation across the target set) stays in the body,
  computed from each resolved target's loaded bundle authz.
- **BundleEnumerate** (List, `@GLOBAL` registry list): no explicit target;
  routing = which bundle/registry to enumerate. Cross-bundle list = authorize
  in dispatch bundle, enumerate the named bundle — the fix for the
  requester-in-target-authz defect.
- **BundleLevel** (Up, Down, PermissionList, PermissionResolve): operate on a
  bundle (+ a permission_request_id); capability `updown` / grant. No target
  resolution; consume the authz stage with zero targets.
- **RelayWide** (NewPeer, ChangePsk, IdentityIntrospect): no bundle; already
  separated via `handle_identity_admin_request` + `authorize_relay_action`
  (requires `all:all`). Subsumed as a recognized mode, not a bypass.

## Reach is data-driven, not a code matrix

There is no code-level policy matrix. Reach for every target operation is the
same uniform check (relationship → scope tier → configured-scope comparison).
What differs per operation is only whether the policy schema *allows* that
capability to be configured to a cross-bundle (`all:all`) scope:

| Capability | Policy-schema allowed scopes | Cross-bundle configurable? |
|------------|------------------------------|----------------------------|
| look       | none / self / all:home / all:all | yes |
| send       | all:home / all:all           | yes |
| list       | all:home / all:all           | yes |
| raww       | none / self / all:home       | no (capped at all:home) |

So `raww` stays intra-bundle today with **no code override**: `raww = all:all`
is not a configurable policy value, so a cross-bundle raww request simply fails
the uniform `all:all` threshold (`authorization_forbidden`). Enabling cross-bundle
raww later is purely a policy-schema change — widen the raww allowed-scope set —
not a routing-layer change.

The only behavior the routing layer *corrects* is `Send`: `authorize_send`
currently uses a `SelfOnly` minimum that always passes, bypassing the threshold
entirely. Applying the uniform threshold makes cross-bundle send require
`all:all`, which the existing `Relay Send Scope Control` spec already mandates
(compliance, not a new contract; BREAKING for permit-all-reliant callers).

## Refactor path (incremental)

- **Step 0:** add `routing` module + `ResolvedRoute`/`ResolvedTarget` and the
  `OperationProfile` table. No behavior change.
- **Step 1:** migrate **Send** (highest existing overlap; it already has the
  target/group shapes). Lowest risk. Tightens cross-bundle send to `all:all`.
- **Step 2:** migrate **Look** (collapses `resolve_look_target_bundle` + the
  `authorize_look` cross_bundle flag into a profile).
- **Step 3:** enable cross-bundle **List** via profile (fixes the
  requester-in-target-authz defect).
- **Step 4:** **Raww** needs no routing-layer code: its policy-schema scope cap
  (`all:home`) makes `all:all` unconfigurable, so cross-bundle raww naturally
  fails the uniform threshold. Enabling it later = widen the raww allowed-scope
  set in the policy schema, no routing-layer change.
- Then `todos/relay/71`: handlers.rs splits along thin bodies; authorization.rs
  splits along (policy loading | capability/profile checks | session
  resolution) — seams this layer creates.

## Risks / open questions

- **Send tightening is BREAKING** for callers relying on permit-all cross-bundle
  send under `all:home`. Mitigated by it matching the published spec; call out
  in release notes.
- Send body concerns (permission deciders, transport/timeout validation) must
  not leak into the layer; the layer's output stays a plain resolved-target set.
- Attribution (authenticated_identity / reserved on_behalf_of) flows unchanged;
  the layer attaches verified identity to each resolved target.
- The exact `OperationProfile` shape and whether profiles are a static table or
  a trait per operation is an implementation detail to settle in Step 0.
