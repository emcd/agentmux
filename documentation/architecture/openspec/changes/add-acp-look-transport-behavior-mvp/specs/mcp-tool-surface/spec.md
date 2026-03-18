## ADDED Requirements

### Requirement: MCP ACP Look Rejection Passthrough

For ACP-backed look targets, MCP SHALL propagate relay-authored
`validation_unsupported_transport` unchanged.

MCP SHALL NOT synthesize alternate error codes for ACP-target look rejection.

#### Scenario: Return unsupported-transport for ACP look target

- **WHEN** caller invokes MCP `look` for ACP-backed target session
- **THEN** MCP returns `validation_unsupported_transport`

#### Scenario: Preserve relay rejection semantics without adapter rewrite

- **WHEN** relay rejects ACP-target look request
- **THEN** MCP propagates relay-authored code/details unchanged
