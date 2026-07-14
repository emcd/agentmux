## Context

Current MCP tools are deliberately crate-private. `McpServer` is
`pub(crate)`, per-tool handlers are registered through
`#[tool_router(... vis = "pub(crate)")]`, parameter structs in
`src/mcp/params.rs` are `pub(super)`, and shared helpers such as relay response
mapping are scoped to the MCP implementation. That is a good internal layout
for a single stdio MCP server, but it is not an embeddable API.

The relay embeddability direction is public-API-first: transports frame input
and call public handlers; the public handlers are the stable boundary. The MCP
tool surface needs the same shape. The stdio MCP server should be one wrapper
over public tool execution, not the only route to execute Agentmux tools.

## Goals

- Let other Rust MCP servers embed Agentmux MCP tools without launching the
  Agentmux stdio server.
- Keep one canonical implementation of each MCP tool contract.
- Keep relay authorization centralized and unchanged.
- Keep strict validation and help/schema generation identical across stdio and
  embedded hosts.
- Publish only intentional request/response/error/context types.
- Keep rmcp router glue and private relay socket frames out of the public API.

## Non-Goals

- Do not add a second authorization path in MCP.
- Do not expose raw `RelayStreamSession` internals as the MCP embedding
  contract.
- Do not make embedders construct relay-verified identity by hand.
- Do not add new tools, aliases, or field migrations.
- Do not stabilize private helper names such as per-handler mapping functions.

## Decisions

- Decision: introduce a public MCP tool host API that accepts explicit MCP
  server association/configuration context and exposes typed async tool calls.
- Decision: the stdio server owns rmcp transport framing, `ToolRouter`
  advertisement, and stdio lifecycle only; it delegates tool execution to the
  public host API.
- Decision: public parameter and response types must be the canonical schema
  types used by both embedded hosts and help/schema generation.
- Decision: public request parameter types must preserve the current schemars
  field overrides and skipped `extra_fields` catch-all pattern that keep
  optional request fields out of `T | null` input-schema unions while still
  rejecting unknown fields.
- Decision: optional response fields keep the existing `skip_serializing_if`
  style semantics where the contract says a field is optional or present only
  when supplied; do not widen public schemas into `T | null` unions unless the
  contract explicitly requires null.
- Decision: embedding association context is the MCP-side carrier for
  relay-authenticated identity and future `on_behalf_of` attribution; embedded
  hosts must use that context rather than inventing a parallel actor channel.
- Decision: relay response mapping is public only at the MCP tool API boundary,
  as typed MCP errors/results. Private helper names and relay wire-frame shapes
  remain internal.
- Decision: embedders may choose a subset of tools to advertise, but invoking a
  public Agentmux tool must use the same validation, request building, relay
  dispatch, and error taxonomy as the stdio server.

## Public API Shape

Implementation may choose exact names, but the public boundary must include:

- an MCP tool host/configuration type built from the same association and relay
  connection inputs used by the stdio server,
- public request parameter types for canonical tools and meta-tool commands,
- public success response types and public MCP tool error payload types,
- public schema/help catalog accessors reused by `help`,
- async tool invocation functions that return typed MCP tool results without
  requiring stdio or rmcp router dispatch.

The public boundary must not include:

- private relay socket Hello/request/response frames,
- rmcp `ToolRouter` construction as the embedding contract,
- internal helper functions that are not stable tool semantics,
- caller-supplied sender/actor identity fields that bypass association-derived
  authority.

## Risks / Trade-offs

- Publishing request and response types constrains future field changes. This is
  intentional for the public MCP API; new contracts should go through OpenSpec.
- Moving parameter structs to public visibility can accidentally publish schema
  details. Mitigate by auditing every field and keeping helper-only state out of
  public structs.
- Rust schema generators may represent `Option<T>` as a null union. The public
  API must preserve the existing request-schema overrides and response omission
  semantics to avoid client and serializer incompatibilities.

## Migration Plan

1. Introduce public MCP host/configuration/context types without changing tool
   behavior.
2. Promote canonical parameter, response, schema, and error types to intentional
   public API modules.
3. Move handler logic behind public async tool functions.
4. Rebuild the stdio `McpServer` and rmcp routers as wrappers over the public
   functions.
5. Add parity tests that call the public API directly and through stdio handler
   paths.
6. Update MCP READMEs and generated docs to distinguish public API from stdio
   transport glue.

## Open Questions

- Exact module names are implementation details as long as public exports are
  intentional and documented.
- Whether admin tools (`updown`, `new`, `change`) are in the stable public MCP
  API follows the separate admin-tool scope decision, not this proposal.
