## Context

Every Agentmux tool is implemented as an rmcp `#[tool]` method on the
crate-private `McpServer`. A handler validates its parameter struct, resolves a
requester session from MCP process association, builds a `RelayRequest`,
dispatches it over `RelayStreamSession`, and assembles a `serde_json::Value`
wrapped in a `CallToolResult`. Validation, relay adaptation, presentation, and
transport all live in one function.

Two consequences follow. First, there is no way to invoke an operation without
the MCP protocol stack. Second, the caller's authority is an MCP configuration
fact: `McpConfiguration::sender_session` and `associated_bundle_paths`,
snapshotted at process startup. Both are reasonable for a stdio server and
neither is meaningful to an in-process caller.

The relay embedding direction is public-API-first: `embeddable-runtime-api`
makes public typed handlers the boundary, gives transports framing duties only,
and produces relay-verified principal context distinct from caller-supplied
identity descriptors. The tool layer sits directly above that runtime and takes
the same shape: canonical operations are the boundary, and MCP is one adapter.

## Goals

- Make canonical typed tool operations the public Rust boundary for Agentmux
  tool semantics.
- Let an in-process application invoke any operation without MCP, stdio, rmcp,
  or an agent harness.
- Derive operation authority from the public runtime's relay-verified principal
  context.
- Keep exactly one implementation of each tool's semantics.
- Keep relay authorization centralized and unchanged.
- Preserve the existing MCP tool contracts byte-for-byte through the adapter.

## Non-Goals

- Do not publish an MCP tool host, MCP association type, or rmcp router as the
  embedding contract.
- Do not add a second authorization path.
- Do not make embedders construct relay-verified identity by hand.
- Do not expose private relay socket frames or `RelayStreamSession` internals.
- Do not add tools, aliases, or field migrations.

## Decisions

- Decision: the public boundary is a set of canonical tool operations with
  typed inputs, typed outputs, and a typed error. The boundary is
  protocol-neutral: no MCP, JSON-RPC, rmcp, stdio, or JSON schema type appears
  in an operation signature, and calling one requires no host or server object.
- Decision: an operation takes the relay-verified principal context produced by
  the public runtime. It does not accept caller-supplied sender identity, and
  there is no operation parameter through which a caller can name a different
  sender.
- Decision: MCP process association is adapter-internal. The MCP adapter
  resolves association into principal context before it calls an operation. A
  direct caller obtains principal context from the runtime. Association is not
  reachable as authorization authority for a direct call.
- Decision: adapter-only failures stay in the adapter. `unassociated_server` is
  an MCP-association failure, so the adapter raises it before delegating and it
  is not a member of the canonical operation error set. The MCP error code and
  details a client sees are unchanged.
- Decision: MCP JSON input schemas, the help catalog, optional-field omission,
  unknown-field rejection at the JSON boundary, and rmcp error mapping are
  presentation concerns owned by the adapter. Schemas are generated from the
  adapter's own MCP parameter types, which keep the schemars and serde derives;
  a canonical operation input carries no schema obligation, since requiring one
  would put an MCP-shaped constraint back on a protocol-neutral type. The
  parameter type maps to the operation input, and a contract test between them
  is what keeps the advertised MCP contract and the operation contract from
  diverging.
- Decision: adapter transport failures stay in the adapter. Timeouts and
  unavailability arising from the MCP adapter's own `RelayStreamSession` socket
  are adapter concerns and are not canonical operation errors; a direct caller
  dispatches through the in-process runtime handler and never traverses that
  socket. Only runtime-handler failures and relay-authored denials enter the
  canonical operation error set.
- Decision: optionality is expressed in the operation types (`Option<T>` and
  typed enums), not in a JSON encoding. The adapter converts an absent optional
  output field into an omitted JSON field, preserving the current
  `skip_serializing_if` behavior, and keeps optional request fields rendering as
  the bare inner JSON type rather than a `[T, "null"]` union.
- Decision: relay remains the centralized authorization decision point.
  Operations validate and adapt, then surface relay denials as canonical
  operation errors carrying the denial code and details unchanged.
- Decision: which tools an adapter advertises stays an adapter concern, governed
  by the per-tool advertisement requirements already in `mcp-tool-surface`. The
  operation layer defines semantics, not advertisement.
- Decision: `help` is adapter-native and gets no canonical operation. Its
  subject matter is the MCP tool catalog and the JSON schemas generated from the
  adapter's parameter types, both of which this design places in the adapter, so
  a canonical `help` operation would have to carry the catalog and schema types
  across a boundary defined to exclude them. It is also the only tool that
  reaches no relay: every other handler builds a `RelayRequest`, and `help`
  answers from `schemars::schema_for!` output plus association status. The
  alternative — a protocol-neutral public catalog descriptor that MCP renders —
  is a coherent design, but it is a new public contract this change does not
  need, so it is out of scope rather than deferred silently.

## Public API Shape

Implementation may choose exact names, but the public boundary must include:

- a typed input type per canonical operation and per meta-tool command,
- a typed success output type per operation,
- one canonical operation error type carrying the code, message, and typed
  details each operation contract defines,
- async operation functions whose parameters are the public runtime handle, the
  relay-verified principal context, and the typed input, returning the typed
  output or the canonical error.

The public boundary must not include:

- rmcp types, including `ToolRouter`, `CallToolResult`, and `ErrorData`,
- MCP association, `McpConfiguration`, or `McpServer` types,
- JSON schema objects or the help catalog as operation inputs or outputs,
- a `JsonSchema` or MCP-deserialization obligation on any operation type,
- private relay socket Hello/request/response frames,
- any caller-supplied sender or actor identity field.

## Risks / Trade-offs

- Keeping the schema derives on the adapter's parameter types buys the
  operation types their protocol neutrality at the cost of a second
  representation of each request contract, and a second of each response
  contract in the JSON rendering. Both can drift. This is the central risk of
  the design and it is paid for with tests rather than structure: contract tests
  bind each parameter type to the operation input it maps to, and parity tests
  assert the MCP payload for a given operation outcome rather than merely that
  both paths succeed. A cheaper structure — deriving schemas straight off the
  operation types — would remove the drift but reimpose an MCP-shaped obligation
  on the protocol-neutral boundary, which is the inversion this change exists to
  undo.
- Publishing operation inputs and outputs constrains future field changes. This
  is intentional for a public API; new contracts go through OpenSpec.
- Operation signatures are shaped by the public runtime's principal context and
  handler contract, so a late change there ripples into every operation. This is
  the reason for the explicit ordering dependency rather than parallel work.
- Moving handler bodies out of `McpServer` risks silently dropping behavior that
  currently lives in the handler — inscription emission, per-tool validation
  order, and relay error mapping. Mitigate by proving each existing MCP
  integration test passes unchanged against the adapter.

## Migration Plan

1. Land `embeddable-runtime-api`'s public runtime handle, public relay handlers,
   and relay-verified principal context.
2. Introduce protocol-neutral operation input, output, and error types derived
   from the current tool contracts.
3. Move each operation-backed handler body — validation, relay request
   construction, dispatch, and outcome interpretation — into an operation
   function taking principal context. `help` stays in the adapter.
4. Reduce each operation-backed MCP handler to: deserialize its MCP parameter
   type, resolve association into principal context, map the parameter type to
   the operation input, call the operation, render the JSON payload, and map the
   canonical error onto the existing MCP error code and details. The `help`
   handler keeps its current shape.
5. Keep help and schema generation in the adapter, generated from the adapter's
   MCP parameter types, and add contract tests binding each parameter type to
   the operation input it maps to.
6. Add direct-call, parity, and authorization tests.
7. Update `src/mcp/README.md` and the subtree READMEs to describe the operation
   boundary and the adapter's remaining role.

## Open Questions

- Exact module and type names are implementation details as long as the
  operation boundary carries no protocol types and principal context stays
  distinct from caller-supplied identity.
- Whether the admin operations (`updown`, `new`, `change`, `drop`) are in the
  first published operation set follows the separate admin-tool scope decision,
  not this proposal.
