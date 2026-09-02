## MODIFIED Requirements

### Requirement: Uniform Cross-Bundle Authorization Model

Target operations SHALL share one fully data-driven authorization model. The
relay SHALL resolve the requester's identity and policy controls in the
requester's **home namespace** — its bound bundle's policy for a session
principal, or the operator policy for a relay-wide principal — classify the
requester-to-target relationship relative to that home namespace, and require a
scope tier on the policy scope ladder:

- self target -> `self`
- same-namespace non-self target -> `home`
- other-namespace target -> `all`

A principal's home namespace SHALL be its native namespace: a session's home is
its bundle, and a relay-wide principal's home is its reserved namespace
(`GLOBAL` / `EXTERNAL` / `RELAY`). `home` SHALL therefore confer authority only
within the principal's own namespace; a relay-wide principal (for example a
`@GLOBAL` operator) SHALL require `all` to reach into any bundle, since a bundle
is not its home namespace. There SHALL be no global/relay-principal exemption
from this threshold.

This requester-axis rule has a target-axis counterpart: a relay-wide (`@GLOBAL`)
*target* SHALL classify at the `home` tier rather than `all`, because it is
delivered through the unified registry in its own namespace rather than by
crossing into a peer bundle. Reaching a relay-wide target — an agent messaging
the operator, or one relay-wide principal messaging another — is therefore not a
cross-namespace act and SHALL NOT demand `all`. This is a routing invariant, not
a per-operation policy exemption.

The relay SHALL then check whether the requester's configured scope for the
operation's capability meets that tier. The relay SHALL NOT apply any
per-operation cross-namespace policy in code; reach SHALL be determined solely by
the requester's configured scope versus the uniform threshold. A peer namespace
SHALL supply only target existence and runtime/transport context; the
requester's membership in the peer namespace SHALL NOT be required, and the
relay SHALL NOT resolve or authorize the requester in a target's (or any other
borrowed) bundle in place of its home namespace, on any target operation.

#### Scenario: Requester authorized in dispatch bundle, not peer bundle

- **WHEN** a session in bundle A issues a cross-bundle operation targeting
  bundle B
- **THEN** relay evaluates the requester's policy controls from bundle A
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Cross-namespace session raww/look authorizes in the home namespace

- **WHEN** a session in bundle A issues a `raww` or `look` targeting a session in
  bundle B
- **AND** the requester's configured scope for that capability in bundle A's
  policy is `all`
- **THEN** relay resolves and authorizes the requester in bundle A (its home
  namespace) and the operation succeeds against the bundle B target
- **AND** relay does not resolve the requester in bundle B and does not return
  `validation_unknown_sender` for a requester unknown there

#### Scenario: Cross-bundle operation denied under home scope

- **WHEN** a requester issues a cross-bundle `look`, `send`, or `list`
- **AND** the requester's configured scope for that capability is `home` or
  narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Cross-bundle list enumerates peer bundle under all-all scope

- **WHEN** a requester with `list = all` lists a configured peer bundle's
  sessions
- **THEN** relay returns the peer bundle's session listing rather than rejecting
  the requester as unknown

#### Scenario: Relay-wide principal needs all-all to reach a bundle

- **WHEN** a relay-wide principal (for example a `@GLOBAL` operator) issues a
  `list` or `send` targeting a bundle namespace
- **AND** its configured scope for that capability is `home`
- **THEN** relay returns `authorization_forbidden`, because the bundle is not the
  principal's home (`GLOBAL`) namespace
- **AND** the same principal under `all` is permitted
