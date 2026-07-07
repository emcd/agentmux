## 1. Apply the spec reconciliation

- [ ] 1.1 Merge the MODIFIED MCP Tool Set and ADDED admin-tool requirement
  blocks into `specs/mcp-tool-surface/spec.md`
- [ ] 1.2 Re-verify the drafted contracts still match `src/mcp/help.rs`,
  `params.rs`, and the relay response variants (BundleTransition / NewPeer /
  ChangePsk) at apply time
- [ ] 1.3 Confirm no code change is required and that the `mcp::updown`
  integration tests still pass (no behavior change)

## 2. Validate and archive

- [ ] 2.1 `openspec validate add-mcp-admin-tool-coverage --strict`
- [ ] 2.2 Archive the change once merged
