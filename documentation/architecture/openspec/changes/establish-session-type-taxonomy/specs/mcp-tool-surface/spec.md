## MODIFIED Requirements

### Requirement: MCP grant relay passthrough taxonomy

MCP `grant` SHALL preserve canonical relay codes and payload semantics
for validation, authorization, and runtime failures, including:

- `validation_invalid_params`
- `validation_cross_bundle_unsupported`
- `authorization_forbidden`
- `runtime_permission_request_already_resolved`
- `runtime_permission_queue_full`
- `runtime_permission_queue_unavailable`

For denied `grant resolve` requests, denial details SHALL preserve
`capability = "grant"`.

#### Scenario: Preserve permission denial capability label

- **WHEN** relay denies `grant resolve` by policy
- **THEN** MCP returns `authorization_forbidden`
- **AND** denial details include `capability = "grant"`

#### Scenario: Preserve already-resolved code

- **WHEN** relay rejects a `grant resolve` because the target request was
  already resolved
- **THEN** MCP returns `runtime_permission_request_already_resolved` unchanged

#### Scenario: Preserve queue-unavailable code

- **WHEN** relay rejects a `grant` request because the persisted queue
  state is unavailable
- **THEN** MCP returns `runtime_permission_queue_unavailable` unchanged
