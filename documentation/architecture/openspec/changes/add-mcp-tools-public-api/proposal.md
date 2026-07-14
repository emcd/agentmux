# Change: Add MCP Tools Public API

## Why

The MCP tool surface is currently implemented as crate-private stdio server
handlers and crate-private parameter structs. That shape prevents other
Agentmux-hosted MCP servers from embedding the same tools without copying
handler code or re-entering through the stdio server.

Agentmux needs the MCP stdio server to become the first host of a public,
typed MCP tool-execution API, parallel to the relay-side
`embeddable-runtime-api` proposal. The public API must preserve the current MCP
tool contracts, relay authorization boundary, help/schema behavior, and strict
validation semantics.

## What Changes

- Define a public Rust API for constructing an MCP tool host and invoking the
  canonical Agentmux MCP tools from embedders.
- Make tool request, response, schema, and error types public only where they
  are part of the embedding contract.
- Move stdio-specific behavior behind a host/transport wrapper over the public
  tool API.
- Preserve relay authorization as the only authorization decision point; MCP
  tool API callers provide verified MCP server association/context, not
  caller-supplied sender identity payloads.
- Preserve strict request validation and JSON schema generation for embedded
  hosts and the stdio server.
- Avoid public exposure of private relay socket frames, private rmcp router
  glue, and internal handler helper functions.

## Non-Goals

- No new MCP tools or tool contract changes.
- No implementation of the active `about` or `do` proposals.
- No public API for private relay Hello/request/response socket frames.
- No independent MCP authorization engine.
- No compatibility aliases for removed or renamed MCP fields.

## Impact

- Affected specs:
  - `mcp-tool-surface`
- Affected code:
  - `src/mcp/params.rs`
  - `src/mcp/help.rs`
  - `src/mcp/errors.rs`
  - `src/mcp/validation.rs`
  - `src/mcp/server/service.rs`
  - `src/mcp/server/handlers/*`
  - MCP integration tests and any crate-level public exports
- Related changes:
  - `embeddable-runtime-api`
  - `add-about-surface-and-description-fields`
  - `add-do-action-tool`
- Source design references:
  - `designs/api/2`
  - `designs/relay-api/1`
