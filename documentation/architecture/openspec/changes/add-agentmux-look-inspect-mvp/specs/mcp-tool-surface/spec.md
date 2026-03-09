## ADDED Requirements

### Requirement: MCP Inspection Naming Exception

The system SHALL expose inspection through MCP tool name `look`.
This SHALL be treated as an explicit and stable exception to delivery tool
naming, where `send` remains reserved for delivery operations.

#### Scenario: Keep inspection separate from send-family semantics

- **WHEN** an MCP client performs session inspection
- **THEN** the client invokes tool `look`
- **AND** inspection is not modeled as an extension of delivery tool `send`

### Requirement: MCP Look Tool

The system SHALL expose a read-only MCP inspection tool named `look`.

`look` SHALL support:

- `target_session` (required session identifier)
- `lines` (optional positive integer)
- `bundle_name` (optional; redundant under associated bundle context)

#### Scenario: Advertise look tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `look`

#### Scenario: Reject invalid lines in look request

- **WHEN** a caller provides `lines` outside valid range
- **THEN** the tool returns `validation_invalid_lines`

#### Scenario: Reject cross-bundle look request in MVP

- **WHEN** a caller provides `bundle_name` that differs from associated bundle
  context
- **THEN** the tool returns `validation_cross_bundle_unsupported`

#### Scenario: Reject unknown target

- **WHEN** caller requests inspection for unknown target session
- **THEN** tool returns `validation_unknown_target`

### Requirement: MCP Look Response Contract

Successful `look` responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_lines` (`string[]`)

`snapshot_lines` ordering SHALL be oldest-to-newest.

#### Scenario: Return canonical inspection payload

- **WHEN** `look` succeeds
- **THEN** response includes canonical inspection payload fields
- **AND** `snapshot_lines` is returned as an array of strings ordered
  oldest-to-newest
