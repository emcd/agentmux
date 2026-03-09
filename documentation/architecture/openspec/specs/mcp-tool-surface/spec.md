# mcp-tool-surface Specification

## Purpose
TBD - created by archiving change add-mcp-tool-surface-contract. Update Purpose after archive.
## Requirements
### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools for the relay MVP:

- `list`
- `send`

The system SHALL NOT expose a temporary `chat` compatibility alias by default.

#### Scenario: Advertise full tool set

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes all required relay tools

### Requirement: Manual Bundle Configuration for MVP

The system SHALL treat bundle definitions as operator-managed configuration in
MVP and SHALL NOT expose MCP tools that mutate bundle configuration.

#### Scenario: Exclude configuration mutation tools from MCP surface

- **WHEN** an MCP client enumerates available tools
- **THEN** tool list excludes bundle mutation operations

### Requirement: Recipient Listing Contract

`list` SHALL return potential recipient sessions from a configured bundle.

Successful `list` responses SHALL include:

- `schema_version`
- `bundle_name`
- `recipients` (array)

Each recipient entry SHALL include:

- `session_name`
- `display_name` (optional)

#### Scenario: List recipients for known bundle

- **WHEN** a caller requests `list` for a known bundle
- **THEN** the system returns configured recipient sessions for that bundle

#### Scenario: Unknown bundle during listing

- **WHEN** a caller requests `list` for a bundle that does not exist
- **THEN** the system rejects the request with `validation_unknown_bundle`

#### Scenario: Include display name when configured

- **WHEN** a recipient has configured display metadata
- **THEN** `list` includes `display_name` for that recipient

### Requirement: Send Target Selection

`send` SHALL support exactly one target mode per request:

- `targets` (non-empty list of recipient identifiers)
- `broadcast=true` for full bundle delivery

`send` SHALL additionally support optional `delivery_mode` with values:

- `async`
- `sync`

If `delivery_mode` is omitted, the system SHALL default to `async`.

`send` SHALL additionally support optional `quiescence_timeout_ms`:

- positive integer milliseconds
- omitted means mode-aware defaults are applied by relay

#### Scenario: Default to async delivery mode

- **WHEN** a caller invokes `send` without specifying `delivery_mode`
- **THEN** the system processes the request using `delivery_mode=async`

#### Scenario: Preserve blocking semantics for explicit sync callers

- **WHEN** a caller invokes `send` with `delivery_mode=sync`
- **THEN** the system returns completion-style outcomes for the request
- **AND** does not downgrade that request to async acceptance semantics

#### Scenario: Reject unknown delivery mode value

- **WHEN** a caller provides a `delivery_mode` value outside `async` or `sync`
- **THEN** the system rejects the request with
  `validation_invalid_delivery_mode`

#### Scenario: Reject invalid quiescence timeout value

- **WHEN** a caller provides `quiescence_timeout_ms` as zero or non-integer
- **THEN** the system rejects the request with
  `validation_invalid_quiescence_timeout`

#### Scenario: Send to explicit subset

- **WHEN** a caller provides `targets` with one or more bundle members
- **THEN** the system attempts delivery only to those targets

#### Scenario: Single target via list mode

- **WHEN** a caller provides one recipient identifier in `targets`
- **THEN** the system treats the request as a valid single-recipient delivery

#### Scenario: Resolve explicit target by configured recipient name

- **WHEN** a caller provides a configured recipient name in `targets`
- **THEN** the system resolves that target to one configured session
- **AND** attempts delivery to that resolved session

#### Scenario: Reject conflicting target modes

- **WHEN** a caller provides `targets` and `broadcast=true` in one request
- **THEN** the system rejects the request with
  `validation_conflicting_targets`

#### Scenario: Reject empty targets list

- **WHEN** a caller provides `targets` as an empty list
- **THEN** the system rejects the request with `validation_empty_targets`

#### Scenario: Allow broadcast with zero effective recipients

- **WHEN** a caller requests `broadcast=true`
- **AND** sender exclusion yields zero effective target sessions
- **THEN** the system treats the request as valid (not a validation error)

### Requirement: Sender Identity Inference

`send` SHALL infer sender identity from the MCP server's configured session
association and SHALL NOT require a sender identity in request payloads.

#### Scenario: Infer sender session identity

- **WHEN** a caller invokes `send`
- **THEN** the system resolves sender identity from MCP server association
- **AND** uses that sender session identity for delivery metadata

#### Scenario: Reject unbound sender identity

- **WHEN** the MCP server instance has no valid session association
- **THEN** the system rejects the request with `validation_unknown_sender`

### Requirement: Send Response Contract

`send` SHALL return a response containing:

- `schema_version`
- `bundle_name`
- `request_id` (when provided by caller)
- `sender_session`
- `sender_display_name` (optional)
- `delivery_mode` (`async` or `sync`)
- `status`
- `results` (per-target entries)

In `delivery_mode=async`, `status` SHALL be `accepted` and each per-target
result SHALL include:

- `target_session`
- `message_id`
- `outcome` = `queued`

In `delivery_mode=sync`, `status` SHALL be one of `success`, `partial`, or
`failure`, and each per-target result SHALL include:

- `target_session`
- `message_id`
- `outcome` (`delivered`, `timeout`, or `failed`)
- `reason` (required when outcome is not `delivered`)

#### Scenario: Return accepted outcome for async request

- **WHEN** a caller invokes `send` with `delivery_mode=async`
- **THEN** the response status is `accepted`
- **AND** per-target outcomes are `queued`

#### Scenario: Return partial outcome for sync mixed delivery

- **WHEN** a caller invokes `send` with `delivery_mode=sync`
- **AND** at least one target succeeds and at least one target fails
- **THEN** `status` is `partial`
- **AND** each target result includes its own outcome and reason data

#### Scenario: Return empty results for zero effective recipients

- **WHEN** a caller invokes `send`
- **AND** effective target resolution yields zero recipients
- **THEN** the response includes `results=[]`
- **AND** `status` is `accepted` for `delivery_mode=async`
- **AND** `status` is `success` for `delivery_mode=sync`

### Requirement: Error Object Contract

Tool failures SHALL return a structured error object with:

- `code`
- `message`
- `details` (optional object)

The system SHALL use stable machine-readable error codes.

#### Scenario: Unknown bundle error

- **WHEN** a caller references a bundle that does not exist
- **THEN** the tool returns error code `validation_unknown_bundle`
- **AND** includes a human-readable message

#### Scenario: Unknown recipient error

- **WHEN** `send` targets a session that is not in the selected bundle
- **THEN** the tool returns error code `validation_unknown_recipient`
- **AND** includes a human-readable message

#### Scenario: Ambiguous recipient name error

- **WHEN** `send` targets a configured recipient name shared by multiple sessions
- **THEN** the tool returns error code `validation_ambiguous_recipient`
- **AND** includes matching session identifiers in error details

### Requirement: MCP Schema Versioning

All successful responses for relay tools SHALL include `schema_version`.

#### Scenario: Include schema version in success response

- **WHEN** any relay MCP tool succeeds
- **THEN** the response includes `schema_version`
