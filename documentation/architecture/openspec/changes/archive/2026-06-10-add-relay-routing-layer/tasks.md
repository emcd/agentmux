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

- [x] 4.1 Implement `BundleEnumerate` resolution stage for List in `routing.rs`
- [x] 4.2 No profile change needed — List already reaches `all:all` via the
      policy schema allowed-scope set; requester resolves in its home bundle
- [x] 4.3 Update `handle_list` to receive `ResolvedRoute`; remove inline
      routing/authz
- [x] 4.4 Validate: `cargo test` passes; no behavior change

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

- [x] 5.1 Migrate Raww onto the shared single-target resolution stage, unifying
      it with Look — the complementary read/write single-target operations. Add a
      `resolve_raww_route` in `routing.rs` that delegates to the shared
      `resolve_target` with relay-wide targets rejected (the same path Look uses),
      and update `handle_raww` to consume the resulting `ResolvedRoute`, removing
      its inline target classification and inline route construction. Standardize
      the relay-wide/reserved target rejection on the generic
      `validation_unsupported_namespace` (Look's behavior): retire Raww's bespoke
      `validation_invalid_params` + `target_class: "ui"` /
      `supported_target_classes` error and update the `raww_rejects_ui_target_class`
      test to expect `validation_unsupported_namespace`. A richer transport-class
      error is deferred to session-attribute-based routing (per-session
      `can_be_written` / `can_be_looked` attributes; see the registry-unification
      idea), which will let Look and Raww share one "target does not accept this
      operation" rejection rather than two near-duplicate inline checks. Sequence
      alongside Step 6 (handle_raww moves during the handlers.rs decomposition) or
      as its own slice; not in the List step. Behavior change is limited to the
      relay-wide raww-target error code.
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

- [x] 6.1 Dissolve `handlers.rs` into a directory module whose `mod.rs` is an
      import-only hub. `handlers/dispatch.rs` holds the per-bundle router and the
      relay-wide entry points; `handlers/sender.rs` holds the shared
      `SenderIdentity` / `resolve_sender_identity`; each operation is its own file
      (`send`, `look`, `raww`, `listing`, `identity`, `permissions`). No
      definitions remain in `mod.rs` (handlers.rs: 1550 lines → a 23-line hub).
- [x] 6.2 Dissolve `authorization.rs` into a directory module along the (policy
      loading | capability/profile checks | session resolution) seams.
      `authorization/context.rs` holds the shared `AuthorizationContext` /
      `PolicyControls` / `PolicyScope` / `UiSessionAuthorization` (members
      `pub(super)` for the sibling seams); `loading` / `resolution` / `checks` are
      the seams; `mod.rs` is an import-only hub (authorization.rs: 1076 lines → a
      30-line hub).
- [x] 6.3 Validate: `cargo test` passes (254 unit + 216 integration); clippy and
      `cargo fmt --check` clean; no behavior change

> **Decomposition vs separation.** Step 5 organized the code along the seams, but
> it is file organization, not layer separation: the bodies still open-code
> `resolve_* → existence → authorize → execute`, and only `Send` is
> namespace-centric. Steps 6–8 make routing/dispatch and authorization genuinely
> separate layers — the goal this proposal set out to deliver ("handlers
> implement op body only"). Added in consultation with the human developer.

## 7. Step 6 — Collapse pre-handler routing onto the home namespace

> **The layer is realized only for `Send`.** `connection.rs` has five dispatch
> branches; only `Send` (`dispatch_send`) is namespace-centric. `Look`/`Raww`
> fall through `resolve_effective_bundle` → `dispatch_request` into a single
> *borrowed* effective bundle — and for `Raww` that borrowed bundle is the
> *target's* (`resolve_raww_routing_bundle`), so the requester is resolved and
> authorized in a namespace that is not its home. This step makes every target
> operation enter on the requester's home-namespace path, like `Send`.

> **Combined with Step 7 in one commit** (Coordinator-approved): the namespace-
> centric handler entries that Step 7 introduces are a prerequisite for routing
> `Look`/`Raww` through the home namespace here — a `GLOBAL` requester has no home
> bundle for the per-bundle dispatcher to load. Tasks 7.1–7.3 and 8.1/8.3 landed
> together.

- [x] 7.1 Route `Look` / `Raww` through the requester's home namespace (its bound
      bundle, or `GLOBAL`) via `dispatch_look` / `dispatch_raww`, never a borrowed
      dispatch/target bundle. Deleted `resolve_effective_bundle` and
      `resolve_raww_routing_bundle`; `resolve_namespace_routing_bundle` is retained
      (and re-documented) solely as the **bundle-subject** resolver for `Up`/`Down`
      / permission ops and the `List` enumerate bundle — those address a bundle the
      requester is a member of, which is not a borrow. (`List` was already
      home/enumerate-split via `dispatch_list`.)
- [x] 7.2 Collapsed the `connection.rs` dispatch branches: `Send` / `Look` / `Raww`
      each have an explicit namespace-centric branch; the residual branch serves
      only bundle-subject ops. No target operation borrows a peer/target bundle.
- [x] 7.3 Validate: `cargo test` passes (256 unit + 216 integration); cross-bundle
      `Look` / `Raww` / `List` resolve the requester in its home namespace
      (`cross_bundle_look_*`, new `cross_bundle_raww_*`, `cross_bundle_list_*`).

## 8. Step 7 — Introduce the dispatch spine; lift resolution/authorization out of bodies

> The three-stage model (Resolution → Authorization → Body) is currently
> open-coded inside each handler: every body calls `resolve_*_route`, validates
> existence, calls `authorize_route`, then executes. The existence-before-authz
> ordering (so `validation_unknown_target` precedes `authorization_forbidden`) is
> upheld four times by convention. This step makes the spine a real pipeline.

- [x] 8.1 Added the namespace-centric entries for the single-target ops —
      `dispatch_look` / `handle_look_routed` and `dispatch_raww` /
      `handle_raww_routed` — mirroring `dispatch_send` / `handle_send_routed`. Each
      loads the requester's home authorization context once (`load_home_context`),
      resolves the `ResolvedRoute`, loads the target's bundle separately
      (`resolve_target_bundle`, the single-target analogue of
      `assemble_delivery_groups`), validates existence, authorizes in the home
      namespace, then runs the body — in that fixed order. Shared sender resolution
      across the home (`resolve_sender_in_namespace`).
- [x] 8.2 Lift the `resolve_*_route` and `authorize_route` calls out of the
      routed handler bodies into a single generic spine
      (`run_target_operation` in `handlers/routed.rs`; folded into the Step 8
      prepare/execute reshape, 9.1).
- [x] 8.3 Validate: `cargo test` passes; error ordering preserved
      (`validation_unknown_target` before `authorization_forbidden`); the
      bare relay-wide raww target now returns the precise
      `validation_unqualified_target` rather than the retired
      `validation_missing_routing_namespace` routing-bundle artifact.

## 9. Step 8 — Execution-only bodies; home-namespace authz for cross-bundle Look/Raww

> With the spine owning resolution and authorization, each operation contributes
> only `prepare` (load per-target bundle configuration, validate existence → a
> delivery plan; the one place configuration is touched) and `execute` (do the
> work). Cross-bundle `Look` / `Raww` load peer configuration in `prepare` —
> exactly as `assemble_delivery_groups` already does for `Send` — so the requester
> is authorized in its **home** namespace, retiring the borrowed-bundle path and
> the residual `home_bundle` (`todos/relay/78`).

- [x] 9.1 Reshape `handle_send` / `handle_look` / `handle_raww` /
      `handle_list_routed` into `prepare` / `execute` closures run by the
      `run_target_operation` spine; neither stage calls `resolve_*` nor
      `authorize_route`, and the resolve → prepare → authorize → execute order
      (existence before authorization) is structural rather than per-handler
      convention
- [x] 9.2 Cross-bundle `Look` / `Raww` load peer-bundle configuration in `prepare`
      (`resolve_target_bundle`), authorizing the requester in its home namespace.
      Eliminated the residual `home_bundle` from the Send delivery path
      (`todos/relay/78`): the non-stream `handle_request` seeds its
      `BundleCatalog` with the home bundle, so `assemble_delivery_groups` resolves
      every bundle-bound target — broadcast and same-namespace included — through
      `ensure_bundle_group` like any peer, and relay-wide (`@GLOBAL`) targets land
      in one uniformly-keyed synthetic `GLOBAL` group regardless of sender.
      Behavior change: stream events delivered to relay-wide UI sessions are now
      attributed to `bundle_name = "GLOBAL"` for bundle-bound senders too
      (previously the sender's home bundle; sender attribution rides in the
      payload's `sender_session`), making them uniform with relay-wide senders —
      two `session_relay_stream` tests updated accordingly.
- [x] 9.3 Added the `session-relay` spec scenario (under the Uniform Cross-Bundle
      Authorization Model requirement): a cross-namespace session→session
      `Raww` / `Look` authorizes in the requester's home namespace and succeeds
      under `all:all`, never resolving the sender in the target's bundle. Also
      retired the requirement's stale "`raww` capped at `all:home`" example,
      which 5.3 obsoleted.
- [x] 9.4 Validate: `cargo test` passes (256 unit + 216 integration); clippy and
      `cargo fmt --check` clean; `openspec validate --strict` passes.
      Routing/dispatch and authorization are separately testable layers — an
      operation body cannot run without the spine having authorized it

> **Out of scope (separate arc).** Per-session routing attributes
> (`can_be_looked` / `can_be_written`) that would let `routing.rs` shed its
> operation-named resolvers depend on the registry-unification idea
> (`ideas/relay/5`) and are not part of this change.
