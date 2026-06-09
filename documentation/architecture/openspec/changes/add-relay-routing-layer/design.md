# Design: Relay Routing/Authz Dispatch Layer

## Context

Source: `designs/relay/7` (full design note with addressing modes, step
sequencing, and open policy questions). Tracks `todos/relay/73` (implement
layer) and unblocks `todos/relay/71` (decompose handlers/authorization).

## Goals / Non-Goals

- Goals: one routing/authz spine for all target operations; **the relay requires
  fully-qualified targets so the resolution stage is config-free (suffix-only
  classification)** — the bare-id convenience moves client-side; cross-bundle
  reach governed by the data-driven tier classification plus the policy schema's
  per-capability allowed-scope set (no per-operation hardcoded policy); handlers
  implement op body only; `todos/relay/76` (cross-bundle raww) resolved as a
  policy-schema change.
- Non-Goals: async rewrite; changing Send's cross-bundle reach (already
  `all:all` in effect via the schema allowed-scope set); per-target partial
  delivery (all-or-nothing is retained; deferred to `todos/relay/77`);
  implementing the client-side fill-in here (cross-lane work in MCP/TUI); ingress
  filter (`ideas/relay/2`, future seam).

## Decisions

### Namespaces, not bundles

The routing-and-authorization unit is the **namespace**, not the bundle. A
bundle *provides* a namespace (named by the bundle name); relay-wide users occupy
the `GLOBAL` namespace. Every principal — sender or target — belongs to exactly
one namespace, derived from its principal-id suffix.

- A **sender's home namespace** is its own: a bundle for a session principal,
  `GLOBAL` for a relay-wide user. Its authorization controls come from that home
  namespace's policy — a bundle's session policy (member presets), or the
  `GLOBAL` operator policy (`load_tui_configuration`, read relay-wide). A sender
  is **never** assigned a borrowed peer namespace.
- **Routing is per-target by namespace**: each target is delivered in the
  namespace its suffix names — a bundle via the catalog, `GLOBAL` via the session
  registry. There is no single "dispatch bundle" that a request routes through.

This corrects today's bundle-thinking: because the op-agnostic dispatcher loads
one `BundleConfiguration`, a `GLOBAL` sender currently borrows the first
bundle-qualified target's namespace just to be dispatched — even though its
controls come from the operator policy and its home is already `GLOBAL`. The
borrowed bundle is inert (the `GLOBAL` sender's identity resolves via the UI
registry and its controls via the relay-wide operator policy, both independent
of which bundle is loaded), and the `validation_missing_routing_namespace` error
for a relay-wide sender with no bundle-qualified target is an artifact of it.

Realizing this means **decoupling authorization-context loading and the
dispatcher from a single bundle** for relay-wide senders: build a `GLOBAL`/
operator authorization context (operator policy + relay-wide permission config,
no bundle members) and route/deliver per-target. `resolve_send_routing_bundle`
is then unnecessary and removed, and a `GLOBAL`→`GLOBAL`-only send succeeds. The
`ResolvedRoute.dispatch_bundle_name` field is the requester's **home namespace**
(a bundle name or `GLOBAL`); a follow-up renames it to `dispatch_namespace` to
match the model.

### Three-stage model

1. **Resolution** (op-agnostic, **config-free**): parse each target's
   `@<namespace>` suffix → `ResolvedTarget { bundle_name, session_id, relay_wide }`.
   No catalog lookup, no configuration access — every target is fully qualified,
   so the suffix alone classifies it (`@GLOBAL` → relay-wide, `@<bundle>` → that
   bundle). A bare target is rejected with `validation_unqualified_target`. This
   stage subsumes the old per-handler target normalization/baring and the
   relay-wide routing-bundle inference. The requester's home namespace is derived
   once from its own principal id (its bundle, or `GLOBAL`).
2. **Authorization**: requester controls come from its **home namespace's
   policy** (a bundle's session policy, or the `GLOBAL` operator policy) — never a
   borrowed peer bundle. The required scope tier is the maximum across the route's
   targets
   (`required_tier`): a peer-bundle target classifies at `all:all`, a same-bundle
   or relay-wide (`@GLOBAL`) target at `all:home`, a self-target at `self`.
   Whether a capability may be configured that high is governed by the schema
   allowed-scope set (`parse_policy_controls`), not a per-operation enum:
   - Send: reaches `all:all` (already enforced; migration only)
   - Look: reaches `all:all` (already enforced; migration only)
   - Raww: reaches `all:all` (was capped; issues/relay/24 flat `authorize_scope`
     path under-enforced at `all:home`, the Raww slice raised it via the schema)
   - List: reaches `all:all` (already enforced; migration only)
3. **Body**: handler receives `ResolvedRoute`; no suffix parsing or policy
   evaluation. It MAY load target bundle configuration for existence validation
   (before authz, preserving the `validation_unknown_target`-before-authz order)
   and for delivery assembly (after authz): members, transport, permission
   deciders, runtime directory. Config loading is delivery work, not routing.

### Addressing modes

- `SingleTarget` (Look, Raww): one resolved target; biggest win — enabling
  cross-bundle becomes a profile flag. Look and Raww are complementary
  (read/write) single-target operations and share one resolution stage
  (`resolve_target`, with relay-wide targets rejected). The relay-wide/reserved
  target rejection is standardized on `validation_unsupported_namespace` for
  both; Raww's original bespoke `target_class: "ui"` error is retired here. A
  richer "this target does not accept this operation" error returns once
  session-attribute-based routing lands (per-session `can_be_looked` /
  `can_be_written`), at which point the operation-named resolvers can collapse
  into attribute checks.
- `MultiTarget` fan-out (Send): N targets across N bundles; per-group
  permission deciders and transport/timeout cross-validation stay in body.
- `BundleEnumerate` (List): no explicit target; routing = which bundle to
  enumerate; cross-bundle list fix comes for free.
- `BundleLevel` / `NoTarget` (Up, Down, Permission ops): use authz stage with
  no resolution targets.
- `RelayWide` (NewPeer, ChangePsk, Identity): subsumed as recognized mode;
  no resolution targets.

### Target qualification moved client-side (root cause)

The relay used to accept bare targets and resolve them itself: a bare id matched
a member of the sender's bound bundle, or — absent a member — a registered
`@GLOBAL` operator (`is_member || has_ui_session(...)` in `resolve_target_groups`),
after `normalize_request_identities` stripped a `@<dispatch-bundle>` suffix to
bare. That leniency is the reason the resolution stage could not be config-free:
classifying a bare target to a bundle vs the relay-wide registry requires the
sender's members and the UI registry, and a bare id silently flips target class
based on configuration the caller cannot see.

The fix relocates the convenience to the client. The relay requires every target
to carry an `@<namespace>` suffix and rejects a bare target with
`validation_unqualified_target`; the MCP server fills the caller's bound bundle
when the user omits it, and the TUI (global user) always qualifies. With
qualification guaranteed, the resolution stage classifies purely from the suffix
and needs no configuration — which is what makes the rest of the layer (a thin,
op-agnostic spine) coherent. This subsumes `normalize_request_identities` (for
targets) and `resolve_send_routing_bundle`.

Cross-lane: the relay's bare-target rejection MUST NOT ship before the MCP/TUI
fill-in, or bare sends break. Coordinator sequences the rollout.

### Send/Look/List are migrations onto the config-free layer

`src/relay/routing.rs` already exists with `OperationProfile`, `ResolvedRoute`,
`ResolvedTarget`, `authorize_route`, and `required_tier`. Send and Look already
call `authorize_route`. Cross-bundle List requester-in-home is already resolved
via the connection.rs `dispatch_list` split. Once qualification is enforced,
these are structural migrations onto the shared layer — no behavior change
beyond the (separately gated) bare-target rejection. The handler keeps existence
validation and delivery assembly (which load configuration); the layer keeps
suffix classification and authorization.

### Authorization granularity: all-or-nothing (per-target deferred)

Authorization is all-or-nothing: any invalid or unauthorized target rejects the
whole operation with `authorization_forbidden`, and no target is delivered. The
check considers **every** target — `required_tier` is the maximum scope tier
across the whole route, never a single representative target. Because the scope
ladder is monotone (`self` ⊆ `all:home` ⊆ `all:all`), a requester whose
configured scope satisfies the maximum is authorized for all targets; the
maximum-tier rule is exactly "authorize every target" expressed in one
comparison. Per-target partial delivery (deliver the allowed targets, reject the
rest individually) is a larger semantic change with its own response-contract
and error-masking trade-offs, deferred to `todos/relay/77`.

### CrossBundlePolicy enum dropped (Option B)

An earlier draft proposed a per-operation `CrossBundlePolicy { Forbidden,
RequireScope(ScopeTier), PermitAll }` table as the cross-bundle authority. It is
not adopted: the shipped spine already decides cross-bundle reach data-driven,
and the four target ops would all be `RequireScope(all:all)`, exactly what
`required_tier` already computes for a peer-bundle target. `required_tier`
classifies the relationship and the policy schema's per-capability allowed-scope
set (`parse_policy_controls`) governs configurability — `grant`/`updown` capped
at `all:home`, `send`/`look`/`raww`/`list` permitted `all:all`. The Raww slice
proved a new cross-bundle capability is a `Capability` variant plus a schema
allowed-scope widening, not a handler edit. Step 0 is therefore spec/design
reconciliation only, with no new enum.

### @GLOBAL → bundle raww requires all:all

`@GLOBAL` user home is the `GLOBAL` namespace, populated by UI sessions that
cannot accept raww. `all:home` covers only `GLOBAL` targets — it is useless for
cross-bundle raww. `all:all` is the only meaningful tier. The flat
`authorize_scope` path in issues/relay/24 under-enforced this by accepting
`all:home`. This proposal corrects it: raww on the route spine requires
`all:all` per the Uniform Cross-Bundle Authorization Model. TUI operator policy
must be updated to `raww = "all:all"` in Step 4 (task 5.5).

## Risks

- **Slice 1 (Send) body concerns**: existence validation, permission deciders,
  and transport/timeout validation stay in the Send body and load configuration;
  the layer output is a plain, config-free resolved-target set. Existence
  validation runs before authorization to preserve the
  `validation_unknown_target`-before-`authorization_forbidden` order.
- **Qualification rollout ordering**: the relay's bare-target rejection is a
  breaking boundary change; it must land with or after the MCP/TUI client-side
  fill-in. Sequenced by the Coordinator across lanes.
- **Slice 3 (List) behavior change**: cross-bundle list callers that previously
  received `validation_unknown_sender` will now receive a real result. Flag in
  release notes.
- **Raww policy-schema widening**: widen `raww` allowed-scope set in
  `parse_policy_controls`; operators must explicitly configure `all:all` to
  enable cross-bundle raww — no silent capability grant.
