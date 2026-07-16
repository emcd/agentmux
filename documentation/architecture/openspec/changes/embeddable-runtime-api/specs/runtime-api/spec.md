## ADDED Requirements

### Requirement: Embeddable Relay Runtime Boundary

The system SHALL expose public Rust relay runtime APIs.
These APIs SHALL cover dispatch, provisioning, and identity introspection and
SHALL be the canonical embedding boundary for both the standalone relay server
and foreign application hosts such as `litrpg`.

The standalone relay server SHALL consume the embeddable runtime API. It SHALL
NOT maintain a separate relay semantics path beside the embeddable API.

#### Scenario: Standalone relay consumes public runtime API

- **WHEN** the standalone relay server receives a relay operation over its
  socket protocol
- **THEN** it frames or decodes the transport input
- **AND** calls the public relay runtime handler for that operation
- **AND** derives routing, authorization, identity, and response semantics from
  the public handler rather than from a standalone-only implementation path

#### Scenario: Foreign host embeds the same runtime API

- **GIVEN** an application host such as `litrpg` configures Agentmux as an
  embedded runtime
- **WHEN** the host submits a typed relay operation through the public API
- **THEN** the same relay runtime semantics used by the standalone relay server
  are applied in-process


### Requirement: Public Dispatch Handler Contract

The system SHALL provide public typed dispatch handlers for relay operations.
Public handlers SHALL accept typed operation inputs plus relay-verified
principal context and SHALL return typed results or typed relay errors.

Public dispatch handlers SHALL NOT require callers to construct private socket
Hello, request, or response frames.

#### Scenario: Embedded host dispatches without socket frames

- **GIVEN** an embedded host has obtained relay-verified principal context
- **WHEN** it calls a public dispatch handler directly
- **THEN** the operation is processed without serializing through private socket
  Hello/request/response frames

#### Scenario: Dispatch requires verified principal context

- **WHEN** a caller invokes a public dispatch handler
- **THEN** the handler requires relay-verified principal context
- **AND** sender or requester fields in the operation payload are not accepted as
  proof of authority


### Requirement: Identity Descriptor and Verified Context Separation

The system SHALL model identity descriptors and verified contexts distinctly.
Caller-supplied identity descriptors and relay-verified principal context SHALL
be distinct public API types.

Caller-supplied identity descriptors SHALL contain principal identifiers and
credential sources. They MAY contain routing defaults and optional metadata.
They SHALL be input to verification or provisioning only. They SHALL NOT be
accepted as verified authorization context for dispatch.

The public identity descriptor credential-source vocabulary SHALL include a
direct in-memory PSK source (`direct_psk`) for host-held credential material.
Embedded hosts SHALL be able to authenticate dynamic agents with `direct_psk`
without forcing those agents through Agentmux-managed credential files or
`socket-trust`.

Relay-verified principal context SHALL be produced by Agentmux verification or
introspection logic and SHALL be the identity authority passed to public
dispatch handlers.

#### Scenario: Descriptor cannot authorize dispatch

- **GIVEN** a caller has a client identity descriptor containing a principal id
- **WHEN** the caller attempts to dispatch a relay operation without verified
  principal context
- **THEN** the system rejects the operation before routing or authorization

#### Scenario: Verification produces dispatch context

- **GIVEN** a caller supplies a valid identity descriptor and credential source
- **WHEN** Agentmux verifies the credential against the relay identity store
- **THEN** Agentmux produces relay-verified principal context suitable for
  public dispatch handlers

#### Scenario: Direct PSK authenticates embedded agent

- **GIVEN** an embedded host holds PSK material for a dynamic agent principal in
  memory
- **WHEN** it supplies an identity descriptor using `direct_psk`
- **THEN** Agentmux verifies the credential through the same identity store path
  used by other credential sources
- **AND** no Agentmux-managed credential file or `socket-trust` fallback is
  required


### Requirement: Configurable Embedded Runtime Roots

The system SHALL accept caller-configured runtime roots.
The embeddable relay runtime SHALL accept caller-configured configuration root
and state root values during initialization.

Agentmux SHALL own its internal runtime, identity, credential-file, event, and
artifact layout beneath the configured state root. Agentmux MUST NOT assume a
fixed parent directory layout, a `litrpg`-specific parent layout, XDG-only root
behavior, or standalone-relay-only root behavior when embedded.

#### Scenario: Embedded host supplies runtime roots

- **GIVEN** an application host initializes Agentmux as an embedded runtime
- **WHEN** it supplies explicit configuration root and state root values
- **THEN** Agentmux loads configuration from the supplied configuration root
- **AND** creates and uses Agentmux-owned runtime artifacts beneath the supplied
  state root
- **AND** does not require the roots to live under any fixed standalone relay or
  application parent directory

#### Scenario: Standalone relay uses the same root contract

- **WHEN** the standalone relay server initializes its public relay runtime
- **THEN** it supplies resolved configuration root and state root values through
  the same runtime initialization contract used by embedded hosts


### Requirement: Public Principal Provisioning Boundary

The system SHALL provide public principal provisioning functions.
These functions SHALL create and update relay principals in the Agentmux
identity store. Provisioning SHALL use the same identity store and verification
path for sessions, application hosts, relays, and dynamic application-created
agents.

Provisioning functions MAY accept optional metadata such as display name and
type hint. Metadata SHALL NOT be treated as authorization authority.

When provisioning returns raw credential material, it SHALL return that material
only in the provisioning response that created or rotated the credential.

Provisioned credentials SHALL be usable through the public identity descriptor
`direct_psk` credential source so embedded agents can authenticate with
host-held in-memory credential material.

#### Scenario: Host provisions dynamic application agent

- **GIVEN** an embedded host needs a dynamic agent principal
- **WHEN** it calls the public principal provisioning API
- **THEN** Agentmux creates or updates the principal record in its identity store
- **AND** returns credential material only as part of that provisioning response
- **AND** later dispatch still requires credential verification into verified
  principal context

#### Scenario: Metadata does not grant authority

- **WHEN** a principal is provisioned with display metadata or a type hint
- **THEN** relay authorization decisions do not treat that metadata as a policy
  grant

#### Scenario: Provisioned credential is usable through direct PSK

- **GIVEN** an embedded host provisions a dynamic agent principal
- **AND** Agentmux returns credential material in the provisioning response
- **WHEN** the host later starts the agent with an identity descriptor using
  `direct_psk`
- **THEN** Agentmux verifies the agent against the provisioned principal record
- **AND** the agent is not required to read an Agentmux-managed credential file
- **AND** the agent is not required to use `socket-trust`


### Requirement: Transport Parity Over Public Handlers

The system SHALL isolate relay semantics from transport mechanics.
Unix socket, MCP, CLI, stdio, and future transports SHALL produce equivalent
relay outcomes by validating or framing their surface input and calling the same
public relay handlers.

Transport adapters MAY own serialization, byte framing, client connection
liveness, stream lifecycle, and surface-specific input parsing. They SHALL NOT
define separate authorization, routing, identity introspection, ACK, or
principal provisioning semantics.

#### Scenario: Surface adapter preserves relay-authored outcome

- **WHEN** a transport adapter receives a valid relay operation from its surface
- **THEN** it calls the corresponding public relay handler
- **AND** propagates the handler's success or typed error outcome without
  re-authorizing or rewriting relay semantics

#### Scenario: Authorization denial is shared across surfaces

- **GIVEN** one verified principal lacks permission for a relay operation
- **WHEN** the same operation is submitted through socket, MCP, CLI, stdio, or
  direct in-process dispatch
- **THEN** each surface returns the same relay-authored authorization outcome
  for that principal and operation


### Requirement: No In-Process Transport Adapter Requirement

The system SHALL allow in-process hosts to call public relay handlers directly.
An embedded host SHALL NOT be required to implement or invoke an in-process
transport adapter whose purpose is to mimic the socket protocol.

#### Scenario: Embedded host bypasses transport adapter layer

- **GIVEN** an embedded host has initialized the public relay runtime and has
  verified principal context
- **WHEN** it submits a relay operation
- **THEN** it calls the public handler directly
- **AND** no private socket frame or in-process socket-like adapter is required


### Requirement: Controlled Delivery Runtime Integration

The embeddable runtime SHALL provide a transport-neutral delivery execution and
observation boundary. An embedder SHALL be able to supply a delivery executor
that receives resolved public delivery input and returns typed delivery outcomes.
An embedder SHALL be able to observe typed delivery lifecycle outcomes without
parsing relay logs or private transport frames.

Agentmux SHALL retain ownership of delivery task construction, worker
registration, receipt generation, outcome correlation, and shutdown gating. The
public runtime contract SHALL NOT expose internal delivery-task fields, worker
registry entries, individual worker-close functions, or terminal-outcome
completion functions for callers to invoke or mutate directly.

The public runtime handle SHALL support controlled shutdown. Once shutdown has
begun, dispatch through public handlers SHALL NOT create or resurrect delivery
workers outside the runtime's shutdown gate, and relay-authored terminal
dispositions SHALL remain observable through the public lifecycle boundary.

#### Scenario: Embedded executor drives a deterministic outcome

- **GIVEN** an embedded host supplies a transport-neutral delivery executor
- **WHEN** a public dispatch handler resolves and submits a delivery
- **THEN** the executor receives resolved public delivery input rather than an
  internal worker task
- **AND** returns a typed delivery outcome through the runtime-owned completion
  path
- **AND** the host can observe the relay-authored lifecycle outcome

#### Scenario: Controlled shutdown preserves runtime invariants

- **GIVEN** an embedded host begins controlled runtime shutdown
- **WHEN** dispatch races with delivery-worker drain
- **THEN** the runtime does not create or resurrect a worker outside its shutdown
  gate
- **AND** records the relay-authored terminal disposition for affected delivery
- **AND** exposes that disposition through the public lifecycle observer

#### Scenario: Public integration does not expose worker internals

- **WHEN** an embedder integrates a delivery executor or lifecycle observer
- **THEN** it does not receive mutable worker registry access
- **AND** it does not construct internal delivery tasks
- **AND** it does not directly close workers or complete task outcomes


### Requirement: Content-Type Envelope Discrimination

The system SHALL use `Content-Type` as the envelope discriminator.
`Content-Type` SHALL be the canonical discriminator for relay envelope payload
semantics.

The following payload classes SHALL be recognized by the relay envelope model:

- `text/plain` for ordinary human-readable messages
- `application/x-agentmux-event+json` for structured Agentmux relay events
- `application/x-agentmux-ext+json` for future extension payloads

`text/plain` SHALL preserve the behavior of current ordinary message delivery.
Extension-specific registry, schema discovery, and submit surfaces SHALL NOT be
defined by this requirement.

#### Scenario: Plain text remains ordinary message delivery

- **WHEN** an envelope has `Content-Type: text/plain`
- **THEN** the relay treats the payload as an ordinary human-readable message
- **AND** current message delivery behavior is preserved

#### Scenario: Agentmux event payload is distinguished

- **WHEN** an envelope has `Content-Type: application/x-agentmux-event+json`
- **THEN** the relay identifies the payload as a structured Agentmux relay event
  rather than ordinary text

#### Scenario: Extension content type is reserved for follow-up proposal

- **WHEN** an envelope has `Content-Type: application/x-agentmux-ext+json`
- **THEN** the relay identifies the payload class as extension traffic
- **AND** extension registration, schema, discovery, and submit rules remain
  governed by a follow-up proposal


### Requirement: Topology-Independent Relay Semantics

The system SHALL preserve topology-independent relay semantics.
Embedded, standalone, and sidecar deployments SHALL preserve equivalent relay
semantics when configuration, identity state, principal context, and requested
operation are equivalent.

Equivalent deployments SHALL produce equivalent routing decisions,
authorization outcomes, identity attribution, envelope Content-Type handling,
ACK behavior, and typed error vocabulary.

#### Scenario: Embedded and standalone send have equivalent semantics

- **GIVEN** equivalent configuration, identity state, and verified principal
  context in embedded and standalone topologies
- **WHEN** the same send operation is submitted in each topology
- **THEN** target resolution, authorization, envelope construction,
  attribution, and response semantics are equivalent

#### Scenario: Sidecar topology does not change relay authority

- **GIVEN** Agentmux runs as a sidecar process for an application host
- **WHEN** the host submits relay operations through a transport adapter
- **THEN** relay authority and outcomes match the equivalent in-process
  embedding case


### Requirement: Accept ACK Timeout Cleanup

The system SHALL bound pending accept-ack correlation.
For any envelope Content-Type whose delivery contract requires an `accept_ack`,
the relay SHALL track pending accept-ack correlation with a bounded timeout.

If `transport_ack` occurs but `accept_ack` does not arrive before the configured
timeout, the relay SHALL record a terminal `accept_ack_timeout` disposition and
SHALL remove the pending correlation. A late `accept_ack` or terminal event for
that correlation SHALL be diagnosed and discarded. It SHALL NOT alter the prior
`accept_ack_timeout` disposition and SHALL NOT recreate pending correlation
state.

This requirement does not make terminal application events mandatory. Terminal
application events, when defined by a Content-Type, remain distinct from
`transport_ack` and `accept_ack`.

#### Scenario: Accept ACK timeout clears pending correlation

- **GIVEN** an envelope Content-Type requires `accept_ack`
- **AND** relay observes `transport_ack` for the envelope
- **WHEN** no `accept_ack` arrives before the configured timeout
- **THEN** relay records terminal disposition `accept_ack_timeout`
- **AND** removes the pending ACK correlation for that envelope

#### Scenario: Late acknowledgement does not resurrect stale state

- **GIVEN** relay removed a pending ACK correlation after
  `accept_ack_timeout`
- **WHEN** a late `accept_ack` or terminal application event arrives for the
  same envelope
- **THEN** relay diagnoses the late signal
- **AND** discards the late signal
- **AND** does not recreate the pending correlation
- **AND** does not alter the prior `accept_ack_timeout` disposition
