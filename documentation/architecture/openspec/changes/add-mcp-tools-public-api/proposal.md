# Change: Add Public Agentmux Tool Operations API

## Why

Agentmux tool semantics — `send`, `look`, `list`, `choose`, `raww`, and the
bundle/credential admin tools — exist today only inside the MCP stdio server.
Handlers are crate-private `#[tool]` methods on `McpServer`, parameter structs
in `src/mcp/params.rs` are `pub(super)`, and the caller's authority is derived
from MCP process association (`sender_session` plus
`associated_bundle_paths`).

An application host such as a Litrpg player agent needs those same operations
in-process. With the current shape its only route is to embed an MCP server:
start `McpServer`, depend on rmcp and `ToolRouter`, satisfy stdio lifecycle
expectations, and manufacture an MCP association whose meaning is a
process-level configuration fact rather than a caller identity. That is a
protocol adapter standing in for an API.

The canonical unit is the tool operation, not its MCP presentation. Making the
operation the public boundary lets a direct caller invoke it with the relay-
verified principal context the public runtime already produces, and reduces MCP
to what it actually is: one adapter that frames requests, maps schemas, and
delegates.

## What Changes

- Define canonical, protocol-neutral Rust tool operations — typed input, typed
  output, typed error — as the public Agentmux boundary, one per tool whose
  subject matter is Agentmux state or behavior. `help` is excluded: its subject
  matter is the MCP tool catalog and generated schemas, so it stays
  adapter-native and gets no operation.
- Require those operations to be callable directly in-process without
  constructing or embedding `McpServer`, without stdio, without rmcp or
  `ToolRouter`, and without an agent harness.
- Require operation authority to be the relay-verified principal context
  produced by the public runtime defined in `embeddable-runtime-api`.
- Make MCP process association an adapter-internal input: the MCP adapter
  resolves it into principal context before delegating, and it is not reachable
  as authorization authority for a direct call.
- Keep MCP JSON input-schema generation, the help catalog, optional-field
  omission, unknown-field rejection at the JSON boundary, and rmcp error mapping
  as adapter concerns. Schemas continue to be generated from the adapter's own
  MCP parameter types, which map to the operation inputs and are contract-tested
  against them, so no operation type carries a schema obligation.
- Keep relay as the single authorization decision point for relay-backed
  operations; operations validate and adapt, then surface relay denials in the
  canonical error taxonomy. MCP adapter transport and association failures stay
  adapter-local and never enter that taxonomy.
- Preserve every existing MCP tool contract: names, request shapes, success
  payloads, omission semantics, schema shapes, and error codes and details.

## Non-Goals

- No new tools and no changes to any existing MCP tool contract.
- No MCP-specific host, server, or association type as the public embedding
  boundary.
- No rmcp types (`ToolRouter`, `CallToolResult`, `ErrorData`) in the public
  operation boundary.
- No second authorization engine; relay remains the decision point.
- No public Rust API for private relay Hello/request/response socket frames.
- No stabilization of private helper names such as per-handler relay response
  mapping functions.
- No compatibility aliases for renamed or removed fields.

## Dependencies and Ordering

- This change depends on `embeddable-runtime-api` and SHALL land after it.
  Operation signatures take the public runtime's relay-verified principal
  context and dispatch through its public relay handlers; neither exists until
  that change lands.
- The dependency is one-directional. `embeddable-runtime-api` does not depend on
  this change.

## Impact

- Affected specs:
  - `tool-operations` (new capability)
  - `mcp-tool-surface`
- Affected code:
  - a new public tool-operations module and its crate-level exports
  - `src/mcp/params.rs`
  - `src/mcp/help.rs`
  - `src/mcp/errors.rs`
  - `src/mcp/validation.rs`
  - `src/mcp/server/service.rs`
  - `src/mcp/server/handlers/*`
  - MCP integration tests
- Related changes:
  - `embeddable-runtime-api` (prerequisite)
- Source design references:
  - `designs/api/2`
  - `designs/relay-api/1`
