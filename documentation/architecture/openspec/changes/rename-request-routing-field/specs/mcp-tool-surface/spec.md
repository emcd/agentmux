## MODIFIED Requirements

### Requirement: MCP Look Tool

The system SHALL expose a read-only MCP inspection tool named `look`.

`look` SHALL support:

- `target_session` (required session identifier)
- `lines` (optional positive integer)
- `namespace` (optional; selects routing context — bundle name or relay-wide
  namespace specifier; redundant under a bundle-bound session context)

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
- `namespace` (optional; selects routing context for target resolution)

`raww` requests SHALL reject caller-supplied sender-like identity fields with
`validation_invalid_params`.

#### Scenario: Reject sender-like field in raww request

- **WHEN** caller submits `raww` request containing sender-like field
- **THEN** MCP rejects request with `validation_invalid_params`

## ADDED Requirements

### Requirement: MCP Send Namespace Parameter

The MCP `send` tool SHALL accept an optional `namespace` parameter that sets
the routing context for the request. When `namespace` is provided, the relay
routes the request in that namespace context (bundle name or relay-wide
specifier). When absent, the routing context defaults to the MCP session's
associated bundle (bundle-bound sessions) or returns an error (relay-wide
sessions without an explicit namespace).

#### Scenario: Send to GLOBAL namespace target

- **WHEN** an MCP caller provides `namespace = "GLOBAL"` and
  `targets = ["operator@GLOBAL"]` in a send request
- **AND** `operator@GLOBAL` is registered as a relay-wide UI session
- **THEN** the send tool delivers the message to that relay-wide connection

#### Scenario: Absent namespace defaults to associated bundle

- **WHEN** an MCP caller omits `namespace` on a send request
- **AND** the MCP session is bundle-bound
- **THEN** the send tool routes in the context of the associated bundle
