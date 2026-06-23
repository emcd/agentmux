# relay-routing-layer Specification

## Purpose
The shared resolution and authorization stages that every target-addressed operation (Send, Look, Raww, List) flows through before reaching its operation-specific body. The spec governs suffix-based target classification (every target MUST carry an `@<namespace>` suffix; bare ids are rejected at the resolution stage without consulting bundle configuration) and uniform cross-bundle authorization evaluated in the requester's home namespace (no per-operation cross-bundle logic; the policy schema's per-capability allowed-scope set is the sole authority for cross-bundle reach). Operation bodies SHALL exclude routing and authorization logic — they receive a fully-resolved and authorized `ResolvedRoute`.
## Requirements
### Requirement: Routing Resolution Stage

The relay SHALL resolve all target-addressed operations (Send, Look, Raww,
List) through a shared, operation-agnostic resolution stage before invoking any
operation handler. The resolution stage SHALL operate **without consulting
bundle configuration or the bundle catalog** — it classifies targets from their
principal-ID suffixes alone — and SHALL:

- Parse each target's `@<namespace>` suffix into a `ResolvedTarget { namespace,
  session_id, relay_wide }`: an `@GLOBAL` suffix marks a relay-wide target; an
  `@<bundle>` suffix names that bundle; the bundle-local session id is the
  portion before the suffix.
- Reject a target that carries no suffix with `validation_unqualified_target`.
  The stage never resolves a bare id against bundle membership or the UI
  registry.
- Identify the requester's dispatch (home) bundle: the sender's bound bundle
  for session principals, or the bundle named by the first bundle-qualified
  target for relay-wide principals.
- Return a `ResolvedRoute { dispatch_namespace, requester_session, targets }`
  to the authorization stage.

Target existence (membership), transport type, and runtime directory are NOT
resolved here; they are bundle-configuration concerns handled by the operation
body after authorization (see Operation Body Contract).

#### Scenario: Single target classified from its suffix

- **WHEN** the relay receives a Look or Raww request targeting `agent@bundle-a`
- **THEN** the resolution stage produces `ResolvedTarget { namespace:
  bundle-a, session_id: agent, relay_wide: false }` without loading bundle
  configuration
- **AND** `dispatch_namespace` is the requester's home bundle

#### Scenario: Multi-target Send classified per target

- **WHEN** the relay receives a Send request with targets in multiple bundles
- **THEN** the resolution stage produces one `ResolvedTarget` per target, each
  classified to its own bundle from its suffix

#### Scenario: Unqualified target rejected at resolution

- **WHEN** a target-addressed request carries a bare target (no `@<namespace>`
  suffix)
- **THEN** the resolution stage returns `validation_unqualified_target` without
  loading any bundle configuration

### Requirement: Authorization Stage

The relay SHALL evaluate authorization for all target-addressed operations
through a shared authorization stage that receives the `ResolvedRoute` from the
resolution stage. The authorization stage SHALL:

- Resolve the requester's policy controls from its **home namespace's** policy:
  a bundle's session policy for a session principal, or the `GLOBAL` operator
  policy for a relay-wide principal. The home namespace is derived from the
  requester's own principal id, never from a target or a borrowed peer bundle. A
  relay-wide (`GLOBAL`) sender is authorized from the operator policy and is
  never assigned a bundle namespace.
- Classify each target's relationship to the requester into a uniform scope tier
  and require the maximum tier across the route: `self` for a self-target,
  `home` for a same-bundle target, `all` for a target in a peer bundle.
  A relay-wide (`@GLOBAL`) target is delivered through the session registry, not
  by crossing into a peer bundle, so it classifies at the `home` tier rather
  than raising the requirement to `all`.
- Consider **every** target when computing the required tier; authorization is
  the maximum across the whole route, never a single representative target.
  Because the scope ladder is monotone, a requester whose configured scope
  satisfies the maximum tier is thereby authorized for every target. The check
  is **all-or-nothing**: if the requester's scope does not satisfy the maximum,
  the entire operation is rejected with `authorization_forbidden` and no target
  is delivered. (Per-target partial delivery is a deferred follow-up.)
- Check the required tier against the requester's configured scope for the
  operation's capability. Each operation contributes only an `OperationProfile`
  (its capability and addressing mode); it carries no per-operation cross-bundle
  policy.

Whether a capability can ever be configured to reach the cross-bundle (`all`)
tier is governed solely by the policy schema's per-capability allowed-scope set
(`parse_policy_controls`). The relay SHALL NOT apply
per-operation cross-bundle logic in handler or routing code; this data-driven
spine — uniform tier classification plus the schema allowed-scope set — SHALL be
the single authority for cross-bundle reach.

#### Scenario: Requester authorized in home namespace for cross-bundle Raww

- **WHEN** a session in bundle A issues a Raww request targeting bundle B
- **THEN** relay evaluates the requester's `raww` policy control from bundle A
  (its home namespace)
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Relay-wide sender authorized from the operator policy

- **WHEN** a relay-wide (`GLOBAL`) sender issues a target-addressed operation
- **THEN** relay resolves the requester's controls from the `GLOBAL` operator
  policy, not from any bundle
- **AND** does not borrow a peer bundle's namespace for the sender

#### Scenario: Relay-wide send to GLOBAL-only targets requires no bundle

- **WHEN** a relay-wide (`GLOBAL`) sender issues a Send whose targets are all
  `@GLOBAL`
- **THEN** relay routes each target through the session registry
- **AND** does not return `validation_missing_routing_namespace`

#### Scenario: Cross-bundle Raww denied under home

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `home` or narrower
- **THEN** relay returns `authorization_forbidden`

Note: for a relay-wide (`@GLOBAL`) principal, `home` covers only the
`GLOBAL` namespace, which is populated exclusively by UI sessions. UI sessions
are rejected by `handle_raww` as an unsupported target class, so `home`
confers zero effective raww reach. `all` is the only meaningful tier for
cross-bundle raww from a relay-wide principal.

#### Scenario: Cross-bundle Raww permitted under all

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `all`
- **THEN** relay routes to the target's bundle and delivers

#### Scenario: Cross-bundle List resolves requester in home bundle

- **WHEN** a session in bundle A issues a List request enumerating bundle B
- **THEN** relay evaluates the requester's `list` policy control from bundle A
- **AND** does not return `validation_unknown_sender` because the requester is
  not a member of bundle B

### Requirement: Operation Body Contract

Operation handler bodies SHALL receive a `ResolvedRoute` whose targets are
already classified (located to a bundle or the relay-wide registry) and
authorized. Handler bodies SHALL NOT:

- Parse `@<namespace>` suffixes from principal IDs.
- Evaluate requester policy controls or classify target scope tiers.

Handler bodies MAY load the target bundles' configuration to validate target
existence and to assemble delivery (member transport, choice deciders,
runtime directory) — this is delivery work, distinct from routing and
authorization. They SHALL implement only operation-specific work: existence
validation, snapshot capture, delivery enqueueing, raw text injection, session
enumeration, or lifecycle control.

#### Scenario: Handler body free of routing and authorization logic

- **WHEN** a developer reads any target-operation handler (`handle_send`,
  `handle_look`, `handle_raww`, `handle_list`)
- **THEN** no principal-ID suffix parsing and no requester-policy or scope-tier
  evaluation are present
- **AND** routing classification and authorization are handled exclusively by
  the dispatch layer, with only existence validation and delivery assembly
  remaining in the body

