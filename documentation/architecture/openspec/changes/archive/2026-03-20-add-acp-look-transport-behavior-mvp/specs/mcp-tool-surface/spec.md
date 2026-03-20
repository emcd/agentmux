## ADDED Requirements

### Requirement: MCP ACP Look Success Passthrough

For ACP-backed look targets, MCP SHALL propagate relay-authored successful look
payloads unchanged, including `snapshot_lines` ordering and emptiness semantics.

MCP SHALL NOT synthesize ACP-specific adapter payloads for look results.

#### Scenario: Return retained ACP snapshot lines from relay response

- **WHEN** caller invokes MCP `look` for ACP-backed target session with retained
  snapshot lines
- **THEN** MCP returns successful look payload
- **AND** `snapshot_lines` are relayed oldest -> newest without reordering

#### Scenario: Preserve empty ACP snapshot semantics

- **WHEN** relay returns successful ACP look payload with `snapshot_lines = []`
- **THEN** MCP propagates `snapshot_lines = []` unchanged
