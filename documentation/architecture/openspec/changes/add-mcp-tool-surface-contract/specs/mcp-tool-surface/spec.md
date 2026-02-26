## ADDED Requirements

### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools for the relay MVP:

- `list`
- `chat`

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

### Requirement: Chat Target Selection

`chat` SHALL support exactly one target mode per request:

- `targets` (non-empty list of session names)
- `broadcast=true` for full bundle delivery

#### Scenario: Send to explicit subset

- **WHEN** a caller provides `targets` with one or more bundle members
- **THEN** the system attempts delivery only to those targets

#### Scenario: Single target via list mode

- **WHEN** a caller provides one session name in `targets`
- **THEN** the system treats the request as a valid single-recipient delivery

#### Scenario: Reject conflicting target modes

- **WHEN** a caller provides `targets` and `broadcast=true` in one request
- **THEN** the system rejects the request with
  `validation_conflicting_targets`

#### Scenario: Reject empty targets list

- **WHEN** a caller provides `targets` as an empty list
- **THEN** the system rejects the request with `validation_empty_targets`

### Requirement: Sender Identity Inference

`chat` SHALL infer sender identity from the MCP server's configured session
association and SHALL NOT require a sender identity in request payloads.

#### Scenario: Infer sender session identity

- **WHEN** a caller invokes `chat`
- **THEN** the system resolves sender identity from MCP server association
- **AND** uses that sender session identity for delivery metadata

#### Scenario: Reject unbound sender identity

- **WHEN** the MCP server instance has no valid session association
- **THEN** the system rejects the request with `validation_unknown_sender`

### Requirement: Chat Response Contract

`chat` SHALL return a synchronous response containing:

- `schema_version`
- `bundle_name`
- `request_id` (when provided by caller)
- `sender_session`
- `sender_display_name` (optional)
- `status` (`success`, `partial`, or `failure`)
- `results` (per-target entries)

Each per-target result SHALL include:

- `target_session`
- `message_id`
- `outcome` (`delivered`, `timeout`, or `failed`)
- `reason` (required when outcome is not `delivered`)

#### Scenario: Return partial outcome for mixed delivery

- **WHEN** at least one target succeeds and at least one target fails
- **THEN** `status` is `partial`
- **AND** each target result includes its own outcome and reason data

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

- **WHEN** `chat` targets a session that is not in the selected bundle
- **THEN** the tool returns error code `validation_unknown_recipient`
- **AND** includes a human-readable message

### Requirement: MCP Schema Versioning

All successful responses for relay tools SHALL include `schema_version`.

#### Scenario: Include schema version in success response

- **WHEN** any relay MCP tool succeeds
- **THEN** the response includes `schema_version`
