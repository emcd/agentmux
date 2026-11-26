## ADDED Requirements

### Requirement: Canonical Tool Operations API

The system SHALL expose canonical Agentmux tool operations as public typed Rust
operation objects that an in-process caller invokes directly: one struct per
operation-backed tool holding the public runtime handle and the sealed
dispatch wiring, with an async invoke method taking the relay-verified
principal context and the typed input.

Invoking an operation SHALL NOT require constructing or embedding the MCP
server, opening a stdio transport, constructing an rmcp `ToolRouter`, running
an agent harness, or presenting any host or server object: the operation
itself is a capability bundle, not a server — it owns no transport endpoint,
resolves no association, generates no schemas, serves no connections, and
cannot act without a per-invocation verified principal.

An operation signature (struct fields, constructor parameters, and invoke
parameters) SHALL carry no MCP, JSON-RPC, rmcp, stdio, or JSON schema type,
and SHALL NOT require a host or server object.

#### Scenario: Application invokes a tool operation in process

- **GIVEN** an application host holds a public runtime handle, the sealed
  dispatch wiring for its owning runtime, and relay-verified principal context
- **WHEN** it constructs the canonical operation object and invokes it with
  a typed input
- **THEN** the operation executes the tool's semantics and returns the typed
  output or the canonical operation error
- **AND** the host does not construct the MCP server, a stdio transport, or an
  rmcp router

#### Scenario: Operation boundary carries no protocol types

- **WHEN** a caller names the struct fields, constructor parameters, input,
  output, and error types of a canonical tool operation
- **THEN** none of those types is an MCP, rmcp, JSON-RPC, or JSON schema type

### Requirement: Tool Operation Authorization Context

A canonical tool operation SHALL take the relay-verified principal context
produced by the public runtime as its authority.

An operation SHALL NOT accept a caller-supplied sender or actor identity as
authorization authority, and no operation input SHALL provide a field through
which a caller names a sender other than the verified principal.

MCP server association SHALL NOT be an authorization input to a direct
operation call.

Relay SHALL remain the centralized authorization decision point for
relay-backed operations. An operation SHALL perform validation and adaptation
only, then surface a relay denial as a canonical operation error carrying the
relay-authored code and denial details unchanged.

#### Scenario: Direct call requires verified principal context

- **WHEN** an in-process caller invokes a canonical tool operation
- **THEN** the operation acts on the relay-verified principal context supplied
  by the public runtime
- **AND** a call without relay-verified principal context is rejected rather
  than executed with inferred authority

#### Scenario: Caller cannot name a different sender

- **WHEN** an in-process caller constructs the input for a canonical tool
  operation
- **THEN** the input offers no field that designates a sender or actor other
  than the verified principal

#### Scenario: MCP association cannot authorize a direct call

- **GIVEN** an MCP server association exists in the process
- **WHEN** an in-process caller invokes a canonical tool operation
- **THEN** that association is not consulted as authorization authority
- **AND** the call is authorized only by relay-verified principal context

#### Scenario: Relay denial surfaces as canonical operation error

- **WHEN** relay denies a request submitted through a canonical tool operation
- **THEN** the operation returns the canonical operation error carrying the
  relay-authored `authorization_forbidden` code and denial details unchanged
- **AND** the operation synthesizes no authorization decision of its own

### Requirement: Canonical Tool Operation Contract Types

Tool operation inputs, outputs, and errors SHALL be the canonical definition of
each operation-backed tool contract. An adapter's presentation types for those
tools SHALL map to them and SHALL be contract-tested against them, and SHALL NOT
impose serialization or schema obligations on the operation types themselves.

A protocol tool that has no canonical operation, because its subject matter is
that protocol's own catalog or schemas, SHALL be governed by its adapter's
contract rather than by this requirement.

Operation types SHALL express optionality in the Rust type system rather than
through a serialization encoding.

The canonical operation error set SHALL be adapter-independent and SHALL NOT
include failures that arise only from an adapter's own association or transport
concerns.

#### Scenario: Absent optional output is absent in the typed output

- **GIVEN** a tool contract documents an output field as present only when
  supplied
- **WHEN** an operation returns an outcome in which that field was not supplied
- **THEN** the typed output represents the field as absent
- **AND** no serialization-level null value is required to express the absence

#### Scenario: Operation error is the same for every consumer

- **GIVEN** two callers reach the same operation, one directly in process and
  one through an adapter
- **WHEN** the operation fails with a given cause
- **THEN** both observe the same canonical operation error code and details

### Requirement: Adapter-Independent Operation Semantics

A canonical tool operation SHALL produce the same outcome for the same typed
input and the same relay-verified principal context, whether it is called
directly or reached through an adapter.

#### Scenario: Direct call and adapted call agree

- **GIVEN** the same relay state and the same relay-verified principal context
- **WHEN** an operation is invoked directly with a typed input, and separately
  through an adapter whose request maps to that same typed input
- **THEN** both invocations produce the same operation outcome
