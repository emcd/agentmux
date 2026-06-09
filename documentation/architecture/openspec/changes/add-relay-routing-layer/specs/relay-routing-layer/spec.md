## ADDED Requirements

### Requirement: Routing Resolution Stage

The relay SHALL resolve all target-addressed operations (Send, Look, Raww,
List) through a shared, operation-agnostic resolution stage before invoking any
operation handler. The resolution stage SHALL:

- Parse the `@<bundle>` suffix from each target principal ID and look up the
  named bundle in the bundle catalog, producing a `ResolvedTarget` for each
  target containing the bundle name, runtime directory, session ID, and
  transport type.
- Identify the requester's dispatch (home) bundle: the sender's bound bundle
  for session principals, or the bundle inferred from the first bundle-qualified
  target for relay-wide principals.
- Return a `ResolvedRoute { dispatch_bundle, targets: Vec<ResolvedTarget> }` to
  the authorization stage.

#### Scenario: Single target resolved to its bundle

- **WHEN** the relay receives a Look or Raww request targeting
  `agent@bundle-a`
- **THEN** the resolution stage produces `ResolvedTarget { bundle: bundle-a,
  session_id: agent, ... }`
- **AND** `dispatch_bundle` is set to the requester's home bundle

#### Scenario: Multi-target Send resolved per target

- **WHEN** the relay receives a Send request with targets in multiple bundles
- **THEN** the resolution stage produces one `ResolvedTarget` per target, each
  located in its own bundle

### Requirement: Authorization Stage

The relay SHALL evaluate authorization for all target-addressed operations
through a shared authorization stage that receives the `ResolvedRoute` from the
resolution stage. The authorization stage SHALL:

- Always resolve the requester's policy controls from the dispatch bundle (the
  requester's home namespace), never from a peer bundle.
- Classify each target's relationship to the requester into a uniform scope tier
  and require the maximum tier across the route: `self` for a self-target,
  `all:home` for a same-bundle target, `all:all` for a target in a peer bundle.
  A relay-wide (`@GLOBAL`) target is delivered through the session registry, not
  by crossing into a peer bundle, so it classifies at the `all:home` tier rather
  than raising the requirement to `all:all`.
- Check the required tier against the requester's configured scope for the
  operation's capability. Each operation contributes only an `OperationProfile`
  (its capability and addressing mode); it carries no per-operation cross-bundle
  policy.

Whether a capability can ever be configured to reach the cross-bundle (`all:all`)
tier is governed solely by the policy schema's per-capability allowed-scope set
(`parse_policy_controls`): `send`, `look`, `raww`, and `list` may be configured
to `all:all`; `grant` and `updown` are capped at `all:home` and can never satisfy
the cross-bundle threshold without a schema change. The relay SHALL NOT apply
per-operation cross-bundle logic in handler or routing code; this data-driven
spine — uniform tier classification plus the schema allowed-scope set — SHALL be
the single authority for cross-bundle reach.

#### Scenario: Requester authorized in dispatch bundle for cross-bundle Raww

- **WHEN** a session in bundle A issues a Raww request targeting bundle B
- **THEN** relay evaluates the requester's `raww` policy control from bundle A
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Cross-bundle Raww denied under all:home

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `all:home` or narrower
- **THEN** relay returns `authorization_forbidden`

Note: for a relay-wide (`@GLOBAL`) principal, `all:home` covers only the
`GLOBAL` namespace, which is populated exclusively by UI sessions. UI sessions
are rejected by `handle_raww` as an unsupported target class, so `all:home`
confers zero effective raww reach. `all:all` is the only meaningful tier for
cross-bundle raww from a relay-wide principal.

#### Scenario: Cross-bundle Raww permitted under all:all

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `all:all`
- **THEN** relay routes to the target's bundle and delivers

#### Scenario: Cross-bundle List resolves requester in home bundle

- **WHEN** a session in bundle A issues a List request enumerating bundle B
- **THEN** relay evaluates the requester's `list` policy control from bundle A
- **AND** does not return `validation_unknown_sender` because the requester is
  not a member of bundle B

### Requirement: Operation Body Contract

Operation handler bodies SHALL receive a `ResolvedRoute` containing targets
already located and authorized. Handler bodies SHALL NOT:

- Parse `@<bundle>` suffixes from principal IDs.
- Perform bundle catalog lookups.
- Evaluate requester policy controls or scope tiers.

Handler bodies SHALL implement only operation-specific work: snapshot capture,
delivery enqueueing, raw text injection, session enumeration, or lifecycle
control.

#### Scenario: Handler body free of routing logic

- **WHEN** a developer reads any target-operation handler (`handle_send`,
  `handle_look`, `handle_raww`, `handle_list`)
- **THEN** no bundle catalog lookups or principal-ID suffix parsing are present
- **AND** routing and authorization are handled exclusively by the dispatch
  layer
