## ADDED Requirements

### Requirement: Uniform Cross-Bundle Authorization Model

Target operations SHALL share one fully data-driven authorization model. The
relay SHALL resolve the requester's policy controls in the requester's dispatch
(home) bundle, classify the requester-to-target relationship, and require a
scope tier on the policy scope ladder:

- self target → `self`
- same-bundle non-self target → `all:home`
- cross-bundle target → `all:all`

The relay SHALL then check whether the requester's configured scope for the
operation's capability meets that tier. The relay SHALL NOT apply any
per-operation cross-bundle policy in code; reach SHALL be determined solely by
the requester's configured scope versus the uniform threshold. A peer bundle
SHALL supply only target existence and runtime/transport context; the
requester's membership in the peer bundle SHALL NOT be required.

Whether a capability can be configured to a cross-bundle (`all:all`) scope SHALL
be governed by the policy schema's per-capability allowed-scope set, not by
relay routing code. A capability whose schema cap is below `all:all` (for
example `raww`, capped at `all:home`) SHALL therefore be unreachable
cross-bundle until the policy schema is widened, with no code override involved.

#### Scenario: Requester authorized in dispatch bundle, not peer bundle

- **WHEN** a session in bundle A issues a cross-bundle operation targeting
  bundle B
- **THEN** relay evaluates the requester's policy controls from bundle A
- **AND** does not require the requester to be a member of bundle B

#### Scenario: Cross-bundle operation denied under home scope

- **WHEN** a requester issues a cross-bundle `look`, `send`, or `list`
- **AND** the requester's configured scope for that capability is `all:home` or
  narrower
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Cross-bundle list enumerates peer bundle under all-all scope

- **WHEN** a requester with `list = all:all` lists a configured peer bundle's
  sessions
- **THEN** relay returns the peer bundle's session listing rather than rejecting
  the requester as unknown

#### Scenario: Capability not configurable to cross-bundle scope fails uniformly

- **WHEN** a requester issues a cross-bundle request for a capability whose
  policy-schema cap is below `all:all` (for example `raww`)
- **THEN** the request fails the uniform `all:all` threshold with
  `authorization_forbidden`
- **AND** no operation-specific code override is involved

## MODIFIED Requirements

### Requirement: Relay Send Scope Control

Relay send authorization SHALL be driven by `send` control scope, evaluated
against the requester's dispatch (home) bundle policy:

- `all:home` allows only same-bundle targets
- `all:all` allows cross-bundle targets

Cross-bundle send SHALL require `all:all`; a cross-bundle send issued under
`all:home` SHALL be rejected with `authorization_forbidden`.

#### Scenario: Reject cross-bundle send under home-only scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "all:home"`
- **THEN** relay returns `authorization_forbidden`

#### Scenario: Permit cross-bundle send under all-all scope

- **WHEN** requester issues cross-bundle send
- **AND** requester policy has `send = "all:all"`
- **THEN** relay routes and delivers to the cross-bundle target(s)
