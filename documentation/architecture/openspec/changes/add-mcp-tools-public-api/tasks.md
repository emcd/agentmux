## 1. Operation Boundary Design

- [ ] 1.1 Inventory each current MCP handler, classify it as operation-backed or
  adapter-native, and split each operation-backed body into operation semantics
  (validation, relay request construction, dispatch, outcome interpretation)
  versus adapter presentation (JSON assembly, schema, rmcp result and error
  construction). `help` is the one adapter-native handler; confirm no other
  handler is adapter-native before relying on that.
- [ ] 1.2 Define the canonical typed input and output for each operation-backed
  tool and meta-tool command from the existing tool contracts, expressing
  optionality in the type system rather than a JSON encoding. Define no
  operation for `help`.
- [ ] 1.3 Define the canonical operation error type covering the code, message,
  and typed details of the current error taxonomy, excluding adapter-only
  failures such as `unassociated_server`.
- [ ] 1.4 Define operation function signatures over the public runtime handle
  and relay-verified principal context from `embeddable-runtime-api`, with no
  caller-supplied sender identity parameter.

## 2. Operation Implementation

- [ ] 2.1 Add the public tool-operations module and its crate-level exports,
  carrying no rmcp, MCP association, or JSON schema types.
- [ ] 2.2 Move each operation-backed handler body into its operation function,
  dispatching through the public relay handlers and preserving existing
  validation order and inscription emission. Leave `help` in the adapter,
  answering from the adapter catalog.
- [ ] 2.3 Map public runtime-handler failures and relay-authored denials onto the
  canonical operation error type, preserving the relay-authored code and
  details. Keep MCP adapter transport and association failures — including
  `validation_unassociated_server`, and `relay_timeout` and `relay_unavailable`
  arising from the adapter's own `RelayStreamSession` socket — out of the
  operation error set; the adapter renders those into its existing MCP errors.
- [ ] 2.4 Keep relay as the only authorization decision point; operations
  perform validation and adaptation only.

## 3. MCP Adapter Reduction

- [ ] 3.1 Reduce each `#[tool]` handler to parameter deserialization,
  association-to-principal-context resolution, operation invocation, payload
  rendering, and error mapping.
- [ ] 3.2 Keep MCP association resolution and its `validation_unassociated_server`
  failure inside the adapter, raised before delegation.
- [ ] 3.3 Render optional output fields with the current omission semantics,
  driven by the operation output's typed optionality.
- [ ] 3.4 Keep JSON input-schema generation and the help catalog in the adapter,
  generated from the adapter's own MCP parameter types and preserving the
  `#[schemars(with = ...)]` field overrides and the flattened `extra_fields` /
  `#[schemars(skip)]` catch-all that reject unknown fields without widening
  optional fields into `[T, "null"]` unions. Keep every schema and
  deserialization derive on those parameter types so no operation input carries
  a schema obligation.
- [ ] 3.5 Implement the mapping from each MCP parameter type to its operation
  input, keeping the parameter type free to differ in representation where the
  MCP wire contract requires it.
- [ ] 3.6 Map the canonical operation error onto the existing MCP error codes and
  details schema, keeping rmcp `ErrorData` construction adapter-local.

## 4. Verification

- [ ] 4.1 Add a direct public-consumer test that imports only the public runtime
  and tool-operation APIs and invokes each operation in-process with
  runtime-supplied principal context, constructing no MCP type and opening no
  MCP transport. Back it with public-surface coverage asserting no operation
  signature names an MCP, rmcp, or JSON schema type. Do not assert that the test
  binary links no rmcp: the crate links it transitively regardless of what the
  caller constructs, so such an assertion would pass vacuously.
- [ ] 4.2 Add parity tests asserting that, for a given operation outcome, the
  MCP-mediated payload equals the documented MCP contract payload for that
  outcome — success and error alike — rather than only that both paths succeed.
- [ ] 4.3 Add authorization tests proving an operation rejects a direct call
  without relay-verified principal context, that no operation input can name a
  different sender, and that MCP association cannot authorize a direct call.
- [ ] 4.4 Add a test that an unassociated MCP adapter still fails with
  `validation_unassociated_server` and that the code is absent from the
  canonical operation error set.
- [ ] 4.5 Add schema tests for optional request fields to prevent `null` union
  widening, and serialization tests for omitted-when-absent response fields.
- [ ] 4.6 Add contract tests between each MCP parameter type and the operation
  input it maps to, so a field added, removed, or retyped on either side without
  the other fails rather than silently diverging. Teeth-check each by breaking
  one side and confirming the test fails. `help` has no operation input, so it
  is excluded from this pairing.
- [ ] 4.7 Add a test that `help` still answers every supported query from the
  adapter catalog with its existing payloads, invoking no canonical operation,
  and that the public tool-operations surface exposes nothing corresponding to
  `help`.
- [ ] 4.8 Run the existing MCP integration tests unchanged, plus
  `cargo fmt --check` and `cargo clippy`.

## 5. Documentation

- [ ] 5.1 Update `src/mcp/README.md` and subtree READMEs to describe the
  operation boundary and the adapter's remaining framing, schema, and mapping
  role.
- [ ] 5.2 Document in-process invocation of the operations, including how a host
  obtains relay-verified principal context from the public runtime.
