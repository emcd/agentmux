# Change: Add relay routing/authz dispatch layer

## Why

The relay accepts **unqualified (bare) target identifiers** and resolves them
itself against bundle configuration: a bare `bob` resolves to a member of the
sender's bound bundle, or — if no member shadows the name — to a registered
`@GLOBAL` operator (`is_member || has_ui_session(...)`). This convenience has two
costs:

- **It couples routing to configuration.** Classifying a bare target to a bundle
  or to the relay-wide registry requires the sender's bundle members and the UI
  registry. A shared, operation-agnostic routing/authorization spine therefore
  cannot classify a target without loading configuration — defeating the purpose
  of a thin routing layer and forcing every handler to re-resolve targets.
- **It is ambiguous.** A bare `bob` silently resolves to a `@GLOBAL` operator
  when no bundle member named `bob` exists — a target-class change driven by
  configuration the caller cannot see.

The bare-id convenience belongs **client-side**: the caller (or its MCP server)
fills in the namespace before the request reaches the relay, and the relay
requires fully-qualified targets. With qualification enforced, target
classification is purely suffix-driven and the routing layer needs no
configuration.

A second, related defect is **bundle-thinking instead of namespace-thinking**.
The routing/authorization unit is the **namespace**: every bundle *provides* a
namespace, and relay-wide users occupy the `GLOBAL` namespace. A sender belongs
to exactly one namespace — its bundle's, or `GLOBAL` — and its authorization
controls come from that home namespace's policy (a bundle's session policy, or
the `GLOBAL` operator policy). Today the op-agnostic dispatcher must load a
single `BundleConfiguration`, so a `GLOBAL` sender is forced to **borrow a peer
bundle's namespace** (the first bundle-qualified target) just to be dispatched —
even though its controls actually come from the relay-wide operator policy and
its home namespace is already `GLOBAL`. The borrowed bundle contributes nothing
the sender uses; the `validation_missing_routing_namespace` error for a
relay-wide sender with no bundle-qualified target is an artifact of that hack,
not a real requirement.

Secondarily, each cross-bundle-capable operation (Send, Look, Raww, List) still
resolves routing and authorization per-handler rather than through a shared
spine, so every new cross-bundle capability requires editing a handler. As the
migrations were delivered, only `Send` became namespace-centric; `Look`/`Raww`/
`List` still enter through `connection.rs`'s `resolve_effective_bundle` and
authorize in a **borrowed** dispatch bundle — and for `Raww` that borrowed bundle
is the *target's* (`resolve_raww_routing_bundle`), so a cross-namespace
session→session `Raww` resolves the sender in the wrong namespace. The
borrowed-bundle anti-pattern this change set out to remove is therefore still
live for the non-`Send` operations. Source: `designs/relay/7`.

## What Changes

- **Require fully-qualified targets (BREAKING).** Every target on a
  target-addressed operation (Send, Look, Raww) MUST carry an `@<namespace>`
  suffix (`@<bundle>` or `@GLOBAL`). The relay rejects a bare target with
  `validation_unqualified_target` and no longer resolves bare ids against bundle
  members or the UI registry.
- **Client-side fill-in (cross-lane).** The MCP server fills the caller's bound
  bundle into a target the user left unqualified; the TUI (global user) always
  qualifies. The bare-id convenience is preserved for users, just moved out of
  the relay. (`mcp-tool-surface`, `tui-surface` — MCP/TUI lanes.)
- **Config-free routing layer.** Because every target is qualified, the shared
  resolution stage in `src/relay/routing.rs` classifies each target from its
  suffix alone — `@GLOBAL` → relay-wide, `@<bundle>` → that bundle — producing a
  `ResolvedRoute` with no catalog or configuration access. This subsumes the
  per-handler target normalization/baring and the relay-wide routing-bundle
  inference (`resolve_send_routing_bundle`, the classification half of
  `resolve_target_groups`).
- **Namespace-centric dispatch (no borrowed bundle).** A sender's home namespace
  is derived from its own principal id (its bundle, or `GLOBAL`); the relay
  resolves its authorization controls from that namespace's policy — a bundle's
  session policy, or the `GLOBAL` operator policy — and never borrows a peer
  bundle. Routing is per-target by namespace (bundle via the catalog, `GLOBAL`
  via the registry). This drops `resolve_send_routing_bundle` and the
  `validation_missing_routing_namespace` artifact, lets a `GLOBAL`→`GLOBAL`-only
  send work, and requires decoupling authorization-context loading and the
  dispatcher from a single `BundleConfiguration` for relay-wide senders.
- **Migrate handlers onto the layer.** Send first, then Look/List/Raww: each
  handler receives a `ResolvedRoute` and keeps only its delivery work. Target
  existence validation and delivery assembly (which do load configuration) stay
  in the handler body — existence before authorization, delivery after —
  preserving error ordering.
- **Authorization stays data-driven** (Step 0): `required_tier` plus the policy
  schema's per-capability allowed-scope set is the single authority for
  cross-bundle reach; no per-operation hardcoded policy. The Raww slice already
  raised `@GLOBAL` → bundle raww to `all:all`.
- Then decompose `handlers.rs` and `authorization.rs` along the clean seams the
  layer creates (`todos/relay/71`).
- **Unify the namespace-centric dispatch spine (completes the layer
  separation).** The decomposition is file organization, not separated layers:
  the bodies still open-code `resolve_* → existence → authorize → execute`, and
  only `Send` is namespace-centric. The spine generalizes `handle_send_routed` to
  every target operation — load the requester's home authorization context,
  resolve the route, run the operation's existence/delivery preparation,
  authorize, then run the body, in that fixed order. Operation bodies reduce to
  `prepare`/`execute` and call neither `resolve_*` nor `authorize_route`;
  `connection.rs`'s per-operation routing (`resolve_effective_bundle`,
  `resolve_raww_routing_bundle`, `resolve_namespace_routing_bundle`) collapses
  onto the home namespace. This retires the borrowed-bundle authz for cross-bundle
  `Look`/`Raww` (a cross-namespace session→session `Raww` is then authorized in
  the requester's home namespace) and the residual `home_bundle`
  (`todos/relay/78`). The proposal is **not complete** until routing/dispatch and
  authorization are separate layers in this sense.

Authorization granularity is **all-or-nothing**: any invalid or unauthorized
target rejects the whole `send`. Authorization considers **every** target (the
maximum required scope tier across the route), not a single representative
target. Per-target partial delivery is deferred to a follow-up todo
(`todos/relay/77`); `design.md` records the rationale.

## Impact

- Affected specs: `relay-routing-layer` (new), `session-relay` (modified),
  `mcp-tool-surface` (modified — client fill-in, MCP lane),
  `tui-surface` (modified — global-user qualification, TUI lane)
- Affected code: `src/relay/routing.rs` (config-free resolution stage),
  `src/relay/handlers/` (per-operation bodies reduced to `prepare`/`execute`),
  `src/relay/connection.rs` (drop `resolve_send_routing_bundle`; collapse
  `resolve_effective_bundle` / `resolve_raww_routing_bundle` /
  `resolve_namespace_routing_bundle` onto the home namespace; one
  namespace-centric dispatch spine), `src/relay/authorization/` (relay-wide/
  operator authorization context not scoped to a borrowed bundle); MCP target
  qualification; TUI target qualification
- **BREAKING** (alpha): the relay rejects bare targets with
  `validation_unqualified_target`; bare ids are no longer resolved against the
  sender's bundle or the UI registry. Clients MUST qualify targets.
- **Sequencing:** client-side qualification (MCP/TUI) MUST land before or with
  the relay's bare-target rejection, or bare sends break. Coordinator sequences
  the cross-lane rollout.
- **BREAKING** (alpha, already shipped): `@GLOBAL` → bundle raww requires
  `raww = "all:all"`; cross-bundle Raww returns `authorization_forbidden` below
  `all:all`.
- Send/Look/List handler migrations: no behavior change beyond target
  qualification.
- **BREAKING** (alpha): once the dispatch spine lands, a cross-namespace
  session→session `Raww`/`Look` is authorized in the requester's **home**
  namespace rather than the target's borrowed bundle. Such a request that the
  borrowed-bundle path silently failed to resolve now resolves and is governed by
  the requester's `raww`/`look` scope (succeeding under `all:all`).
