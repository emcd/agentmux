## ADDED Requirements

### Requirement: Advertise MCP raww tool

MCP tool inventory SHALL advertise top-level tool `raww` for direct single-
target raw writes.

#### Scenario: Include raww in tool inventory

- **WHEN** MCP client requests tool catalog
- **THEN** catalog includes `raww`

### Requirement: MCP raww request contract

MCP `raww` request fields for MVP SHALL be:
- `target_session` (required)
- `text` (required)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)

`raww` requests SHALL reject caller-supplied sender-like identity fields with
`validation_invalid_params`.

#### Scenario: Reject sender-like field in raww request

- **WHEN** caller submits `raww` request containing sender-like field
- **THEN** MCP rejects request with `validation_invalid_params`

### Requirement: MCP raww sender authority

MCP raww sender identity SHALL be association-derived from MCP server context
and SHALL NOT be caller-overridable.

#### Scenario: Use association-derived sender for raww

- **WHEN** caller invokes MCP `raww`
- **THEN** MCP resolves sender principal from associated session context
- **AND** uses that principal for relay authorization/evaluation

### Requirement: MCP raww relay passthrough taxonomy

MCP raww SHALL preserve canonical relay codes and payload semantics for
validation and authorization failures, including:
- `validation_unknown_target`
- `validation_cross_bundle_unsupported`
- `validation_invalid_params`
- `authorization_forbidden`

For denied raww requests, denial details SHALL preserve
`capability = "raww.write"`.

#### Scenario: Preserve raww denial capability label

- **WHEN** relay denies raww by policy
- **THEN** MCP returns `authorization_forbidden`
- **AND** denial details include `capability = "raww.write"`

### Requirement: MCP raww success payload contract

MCP raww success responses SHALL preserve relay acceptance payload contract.

Required success fields:
- `status` (value `accepted`)
- `target_session`
- `transport`

Optional fields:
- `request_id`
- `message_id`
- `details`

For ACP accepted responses, MCP SHALL preserve
`details.delivery_phase = "accepted_in_progress"` unchanged.

#### Scenario: Preserve ACP accepted-in-progress detail for raww

- **WHEN** relay returns successful ACP raww response with
  `details.delivery_phase = "accepted_in_progress"`
- **THEN** MCP returns same `details.delivery_phase` unchanged
