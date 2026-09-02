## MODIFIED Requirements

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
  `home` for a same-namespace target, `all` for a target in a peer namespace. A
  relay-wide (`@GLOBAL`) target is delivered through the unified registry in its
  own namespace, not by crossing into a peer bundle, so it classifies at the
  `home` tier rather than raising the requirement to `all`.
- Consider **every** target when computing the required tier; authorization is
  the maximum across the whole route, never a single representative target.
  Because the scope ladder is monotone, a requester whose configured scope
  satisfies the maximum tier is thereby authorized for every target. The check is
  **all-or-nothing**: if the requester's scope does not satisfy the maximum, the
  entire operation is rejected with `authorization_forbidden` and no target is
  delivered. (Per-target partial delivery is a deferred follow-up.)
- Check the required tier against the requester's configured scope for the
  operation's capability. Each operation contributes only an `OperationProfile`
  (its capability and addressing mode); it carries no per-operation
  cross-namespace policy.

The relay SHALL NOT apply per-operation cross-namespace logic in handler or
routing code. This data-driven spine — uniform tier classification checked
against the requester's configured scope — SHALL be the single authority for
cross-namespace reach.

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
- **THEN** relay routes each target through the unified registry
- **AND** does not return `validation_missing_routing_namespace`

#### Scenario: Cross-bundle Raww denied under home

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `home` or narrower
- **THEN** relay returns `authorization_forbidden`

Note: for a relay-wide (`@GLOBAL`) principal, `home` covers only the `GLOBAL`
namespace, which is populated by relay-wide sessions. Sessions whose registry
entry carries `can_be_written = false` are rejected by the raww capability gate,
so `home` confers no effective raww reach to those targets. `all` remains the
meaningful tier for cross-bundle raww from a relay-wide principal.

#### Scenario: Cross-bundle Raww permitted under all

- **WHEN** a requester issues a Raww request to a target in a different bundle
- **AND** the requester's configured `raww` scope is `all`
- **THEN** relay routes to the target's bundle and delivers

#### Scenario: Cross-bundle List resolves requester in home bundle

- **WHEN** a session in bundle A issues a List request enumerating bundle B
- **THEN** relay evaluates the requester's `list` policy control from bundle A
- **AND** does not return `validation_unknown_sender` because the requester is not
  a member of bundle B
