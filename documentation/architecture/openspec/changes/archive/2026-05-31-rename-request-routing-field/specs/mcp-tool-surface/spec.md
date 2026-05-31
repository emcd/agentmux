## MODIFIED Requirements

### Requirement: MCP Look Tool

The system SHALL expose a read-only MCP inspection tool named `look`.

`look` SHALL support:

- `target_session` (required session identifier)
- `lines` (optional positive integer)

#### Scenario: Advertise look tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `look`

#### Scenario: Reject invalid lines in look request

- **WHEN** a caller provides `lines` outside valid range
- **THEN** the tool returns `validation_invalid_lines`

#### Scenario: Reject unknown target

- **WHEN** caller requests inspection for unknown target session
- **THEN** tool returns `validation_unknown_target`

## MODIFIED Requirements

### Requirement: MCP raww request contract

MCP `raww` request fields SHALL be:
- `target_session` (required)
- `text` (required)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)

`raww` requests SHALL reject caller-supplied sender-like identity fields with
`validation_invalid_params`.

#### Scenario: Reject sender-like field in raww request

- **WHEN** caller submits `raww` request containing sender-like field
- **THEN** MCP rejects request with `validation_invalid_params`
