## ADDED Requirements

### Requirement: MCP Tools Public Rust API

The system SHALL expose a public Rust API for hosting and invoking the canonical
Agentmux MCP tools without requiring the Agentmux stdio MCP server process.

The public API SHALL preserve the same request validation, request-to-relay
mapping, success response contracts, error taxonomy, help/schema behavior, and
relay authorization pass-through semantics used by the stdio MCP server.

#### Scenario: Embedded MCP server invokes Agentmux tool

- **GIVEN** another Rust MCP server embeds Agentmux MCP tools through the public
  API
- **WHEN** it invokes a canonical Agentmux tool with the same MCP association
  context and request payload as the stdio server
- **THEN** the public API applies the same validation and relay dispatch
- **AND** returns the same success or error payload contract as the stdio MCP
  server

#### Scenario: Stdio server delegates to public API

- **WHEN** the Agentmux stdio MCP server receives a tool call
- **THEN** it frames the MCP request through rmcp/stdin/stdout transport glue
- **AND** delegates canonical tool execution to the same public API used by
  embedded hosts

### Requirement: Public MCP Tool Contract Types

The system SHALL publish intentional Rust types for canonical MCP tool request
parameters, success responses, error payloads, and help/schema metadata where
those types are part of the MCP embedding contract.

Public request types SHALL preserve strict unknown-field rejection for MCP tool
payloads and meta-tool argument payloads. Public response types SHALL preserve
the documented optional-field serialization behavior for each tool contract.

Generated tool input schemas for optional request fields SHALL render the bare
inner JSON type accepted by the tool contract and SHALL NOT advertise a
`[T, "null"]` union unless the tool contract explicitly accepts JSON `null`.

#### Scenario: Public parameter type rejects unknown fields

- **GIVEN** an embedded host deserializes a public MCP request parameter type
- **WHEN** the payload contains a field not accepted by the tool contract
- **THEN** the public API rejects the request with `validation_invalid_params`
- **AND** does not silently drop the unknown field

#### Scenario: Public response type omits absent optional field

- **GIVEN** a canonical MCP response field is documented as optional or present
  only when provided
- **WHEN** the public API serializes a response where that field is absent
- **THEN** the field is omitted unless the tool contract explicitly requires a
  JSON `null` value

#### Scenario: Public optional request schema suppresses null unions

- **GIVEN** a canonical MCP request parameter field is optional and has an inner
  type such as `string`, `integer`, or `boolean`
- **WHEN** stdio MCP or an embedded host generates the tool input schema from
  the public request type
- **THEN** the generated schema advertises the bare inner JSON type for that
  field
- **AND** does not advertise a `[T, "null"]` union unless the tool contract
  explicitly accepts JSON `null`

### Requirement: MCP Stdio Wrapper Boundary

The system SHALL treat rmcp router construction, stdio lifecycle handling, and
MCP transport framing as wrapper concerns over the public MCP tools API rather
than as the public Rust embedding boundary.

The public MCP tools API SHALL NOT require embedders to instantiate the stdio
server, depend on private `ToolRouter` construction, or route calls through
private relay socket frame structs.

#### Scenario: Embedder bypasses stdio transport

- **GIVEN** an embedded host has configured the public MCP tools API
- **WHEN** it invokes an Agentmux MCP tool
- **THEN** it does not need to launch the stdio MCP server
- **AND** it does not serialize through private stdio or relay socket frames

#### Scenario: Router glue remains transport-specific

- **WHEN** the stdio MCP server advertises tools to an MCP client
- **THEN** rmcp router glue is used only to expose the tools over stdio
- **AND** the tool semantics remain defined by the public MCP tools API

### Requirement: MCP Public API Authorization Boundary

The public MCP tools API SHALL derive sender and actor authority from explicit
MCP server association or relay-verified context and SHALL NOT accept
caller-supplied sender-like payload fields as authorization authority.

Relay SHALL remain the centralized authorization decision point for relay-backed
MCP tools. The public MCP tools API SHALL perform validation and adaptation, then
pass through relay authorization denials with the canonical MCP error taxonomy.

#### Scenario: Caller cannot override sender through public API payload

- **WHEN** a public MCP API caller includes a sender-like identity field in a
  tool payload
- **THEN** the public API rejects the request according to the canonical MCP
  validation contract
- **AND** does not use that field as authorization authority

#### Scenario: Relay denial passes through public API

- **WHEN** relay denies a request submitted through the public MCP tools API
- **THEN** the public API returns the same `authorization_forbidden` code and
  denial detail schema as the stdio MCP server

### Requirement: MCP Embedding Tool Inventory Consistency

The public MCP tools API SHALL expose tool inventory and help/schema metadata
from the same canonical catalog used by the stdio MCP server.

Embedded hosts MAY choose not to advertise every available Agentmux tool, but
any advertised Agentmux tool SHALL use the canonical name, request schema,
response contract, and error taxonomy for that tool.

#### Scenario: Embedded host advertises subset of Agentmux tools

- **GIVEN** an embedded host chooses to advertise only a subset of Agentmux MCP
  tools
- **WHEN** it advertises one of those tools
- **THEN** the advertised tool name and schema match the canonical Agentmux MCP
  catalog
- **AND** invoking the tool follows the canonical Agentmux MCP contract

#### Scenario: Help schema comes from canonical public type

- **WHEN** either stdio MCP or an embedded host requests schema metadata for an
  Agentmux tool
- **THEN** the schema is generated from the same public parameter type used for
  tool invocation
