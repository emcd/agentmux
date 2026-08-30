## ADDED Requirements

### Requirement: MCP Tool Adapter Delegation

MCP SHALL execute the semantics of every operation-backed tool by delegating to
the canonical Agentmux tool operations, and SHALL derive each such tool's result
from the operation outcome it receives.

An operation-backed tool is one whose subject matter is Agentmux state or
behavior. The `help` tool is not operation-backed, because its subject matter is
the MCP adapter's own tool catalog and generated JSON schemas; it is governed by
the MCP Help Adapter Ownership requirement instead.

MCP SHALL NOT carry a second implementation of an operation-backed tool's
request validation against relay contracts, relay dispatch, or outcome
interpretation.

Delegation SHALL preserve every existing operation-backed MCP tool contract:
tool names, request shapes, success payload fields and ordering semantics,
optional-field omission, generated input schemas, and error codes and details.
Preservation of the `help` contract lives under the MCP Help Adapter Ownership
requirement.

#### Scenario: Stdio tool call delegates to the canonical operation

- **WHEN** the Agentmux stdio MCP server receives a call for an operation-backed
  tool
- **THEN** it deserializes the request, resolves its own association into
  relay-verified principal context, and invokes the canonical tool operation
- **AND** it renders its response from that operation's outcome

#### Scenario: MCP tool contract is unchanged by delegation

- **GIVEN** an MCP client issues a tool call that succeeded before delegation
- **WHEN** the same call is issued after MCP delegates to the canonical
  operation
- **THEN** the client observes the same tool name, success payload, and field
  omission behavior

### Requirement: MCP Help Adapter Ownership

The `help` tool SHALL be an MCP adapter-native introspection tool rather than a
canonical Agentmux tool operation.

`help` SHALL answer from the adapter's own tool catalog and the schemas
generated from the adapter's MCP parameter types, and SHALL NOT invoke a
canonical tool operation. There SHALL be no canonical operation for `help`, and
`help` SHALL NOT appear in the public tool-operations surface.

The existing `help` contract SHALL be preserved unchanged: its query modes,
returned inventory and schema payloads, association-status reporting, and its
`validation_invalid_params` failure for unknown queries.

#### Scenario: Help answers from the adapter catalog

- **WHEN** an MCP client calls `help` with any supported query
- **THEN** the adapter answers from its own tool catalog and generated schemas
- **AND** it invokes no canonical tool operation
- **AND** the returned payload matches the existing `help` contract for that
  query

#### Scenario: Help is absent from the public operation surface

- **WHEN** an in-process caller enumerates the public tool operations
- **THEN** no operation corresponds to `help`
- **AND** no operation input, output, or error type carries the MCP tool catalog
  or a generated JSON schema

### Requirement: MCP Adapter Presentation Boundary

The MCP adapter SHALL own JSON input-schema generation, the help catalog,
optional-field omission in serialized payloads, unknown-field rejection at the
JSON boundary, rmcp result construction, and rmcp error mapping.

The adapter's parameter types for operation-backed tools SHALL map to the typed
operation inputs, and every MCP schema SHALL be generated from the adapter's own
parameter types. A canonical tool operation input SHALL NOT be required to carry
MCP schema or MCP deserialization artifacts, and no rmcp type SHALL appear in a
canonical tool operation signature.

Where a parameter type maps to an operation input, that correspondence SHALL be
contract-tested rather than asserted, so that a divergence between the
advertised MCP contract and the operation contract fails. The `help` parameter
type maps to no operation input and is exempt from that pairing.

#### Scenario: Optional request field schema stays a bare inner type

- **GIVEN** an MCP parameter field is optional with an inner type such as
  `string`, `integer`, or `boolean`
- **WHEN** the MCP adapter generates the tool input schema from its MCP
  parameter type
- **THEN** the generated schema advertises the bare inner JSON type
- **AND** does not advertise a `[T, "null"]` union unless the tool contract
  explicitly accepts JSON `null`

#### Scenario: Parameter type divergence from the operation input fails

- **GIVEN** an MCP parameter type maps to a canonical operation input
- **WHEN** either side gains, loses, or retypes a field so the mapping no longer
  covers the tool contract
- **THEN** the contract test between them fails

#### Scenario: Adapter rejects unknown JSON fields

- **WHEN** an MCP tool payload or meta-tool argument payload carries a field the
  tool contract does not accept
- **THEN** the adapter rejects the request with `validation_invalid_params`
- **AND** does not silently drop the unknown field

#### Scenario: Adapter omits an absent optional output field

- **GIVEN** an operation outcome in which an optional output field is absent
- **WHEN** the MCP adapter serializes the response
- **THEN** the field is omitted from the JSON payload rather than serialized as
  `null`, unless the tool contract explicitly requires a JSON `null` value

#### Scenario: Canonical operation error maps to the existing MCP error

- **WHEN** a canonical tool operation returns a canonical operation error
- **THEN** the MCP adapter maps it onto the MCP error code and details schema
  that tool contract already defines
- **AND** rmcp error construction remains inside the adapter

### Requirement: MCP Advertised Tool Fidelity

An MCP adapter SHALL advertise each operation-backed Agentmux tool under the
canonical tool name, input schema, response contract, and error taxonomy defined
by that tool's operation contract.

An adapter MAY advertise a subset of the available Agentmux tools. An adapter
SHALL NOT advertise a renamed, reshaped, or partially implemented variant of an
Agentmux tool.

Advertised schema and help metadata for an operation-backed tool SHALL be
generated from the adapter's MCP parameter types, and those types SHALL be
contract-tested against the operation inputs they map to rather than maintained
independently of them.

The `help` tool is governed by the MCP Help Adapter Ownership requirement. It
has no operation contract, no operation input, and no parameter-to-operation
mapping; its schema and catalog behavior is its own preserved adapter contract,
and this requirement imposes no operation mapping on it.

#### Scenario: Adapter advertises a subset of Agentmux tools

- **GIVEN** an MCP adapter advertises only some of the available operation-backed
  Agentmux tools
- **WHEN** it advertises one of them
- **THEN** the advertised name is that tool's canonical name and the advertised
  input schema accepts exactly that tool's canonical request contract
- **AND** invoking it follows that tool's canonical request, response, and error
  contract

#### Scenario: Help metadata comes from the mapped parameter type

- **WHEN** an MCP adapter reports schema metadata for an operation-backed
  Agentmux tool
- **THEN** the metadata is generated from the same MCP parameter type the
  adapter deserializes and maps to that tool's operation input
- **AND** it is not a separately maintained description that can drift from
  either the parameter type or the operation contract

### Requirement: MCP Association Is Adapter Internal

MCP server association SHALL be internal to the MCP adapter, including the
associated bundle and sender session resolved at MCP startup.

The adapter SHALL resolve association into relay-verified principal context
before invoking a canonical tool operation, and SHALL NOT pass association
itself to an operation as authorization authority.

Association failures SHALL remain adapter-level failures and SHALL NOT be
members of the canonical operation error set.

#### Scenario: Unassociated MCP server still fails with the existing code

- **GIVEN** the MCP server has no associated bundle or sender session
- **WHEN** a client invokes a tool that requires association
- **THEN** the adapter returns `validation_unassociated_server` with its
  existing details and remedy text
- **AND** it does not invoke the canonical tool operation

#### Scenario: Association is absent from the operation boundary

- **WHEN** an in-process caller invokes a canonical tool operation
- **THEN** it supplies no MCP association value
- **AND** `validation_unassociated_server` is not a reachable outcome of that
  call
