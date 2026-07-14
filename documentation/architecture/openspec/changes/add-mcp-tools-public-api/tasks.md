## 1. API Design

- [ ] 1.1 Inventory current crate-private MCP types and helpers that are tool
  contract versus implementation detail.
- [ ] 1.2 Define public module boundaries for tool host, parameters, responses,
  errors, and help/schema accessors.
- [ ] 1.3 Decide public names for MCP association/context types without exposing
  private relay socket frames or rmcp router internals.

## 2. Implementation

- [ ] 2.1 Introduce the public MCP tool host API with typed async invocation
  methods.
- [ ] 2.2 Promote canonical parameter and response types to public API modules,
  preserving strict unknown-field validation, the `#[schemars(with = ...)]`
  request-field overrides, the flattened `extra_fields` / `#[schemars(skip)]`
  catch-all layer, and omission semantics for optional response fields.
- [ ] 2.3 Move shared request validation and relay response mapping behind the
  public API boundary.
- [ ] 2.4 Refactor stdio `McpServer` and per-tool rmcp handlers to delegate to
  the public API.
- [ ] 2.5 Preserve `help` schema generation from the same public parameter types
  used by embedded callers.
- [ ] 2.6 Keep relay authorization pass-through behavior unchanged.

## 3. Verification

- [ ] 3.1 Add direct public-API tests for each supported tool and meta-tool
  command.
- [ ] 3.2 Add parity tests proving stdio/rmcp handlers and public API calls
  produce the same success and error payloads.
- [ ] 3.3 Add schema tests for optional request fields to prevent unintended
  `null` union widening in generated tool input schemas, and add serialization
  tests where needed for omitted-when-absent response fields.
- [ ] 3.4 Run `cargo fmt --check`, `cargo check`, and MCP integration tests.

## 4. Documentation

- [ ] 4.1 Update `src/mcp/README.md` and subtree READMEs to describe the public
  API boundary and stdio wrapper role.
- [ ] 4.2 Update public crate documentation or examples for embedding Agentmux
  MCP tools in another MCP server.
