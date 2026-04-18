## 1. Contract Relock

- [x] 1.1 Relock MCP top-level listing tool from `list.sessions` to `list`.
- [x] 1.2 Require explicit `command="sessions"` for MVP list requests.
- [x] 1.3 Preserve existing selector semantics (`bundle_name` vs `all`,
  associated/home default, all-mode fanout behavior).
- [x] 1.4 Keep canonical list response payload shapes unchanged.
- [x] 1.5 Lock pre-stable breaking posture: no compatibility shim.

## 2. Implementation

- [x] 2.1 Update MCP list tool registration/validation in `src/mcp/mod.rs`.
- [x] 2.2 Update MCP integration tests for tool catalog and list invocation
  shape.
- [x] 2.3 Update MCP README documentation for relocked tool topology.
- [x] 2.4 Keep relay/CLI surfaces unchanged in this slice.

## 3. Validation

- [x] 3.1 `cargo check --all-targets --all-features`
- [x] 3.2 `cargo test --all-features --test integration mcp::list::`
- [x] 3.3 `openspec validate update-mcp-list-meta-tool-topology-mvp --strict`
