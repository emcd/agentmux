## MODIFIED Requirements

### Requirement: MCP Look Tool

The system SHALL expose a read-only MCP inspection tool named `look`.

`look` SHALL support:

- `target_session` (required session identifier)
- `lines` (optional positive integer)

Routing context for `look` SHALL be inferred from the `@<namespace>` suffix of
`target_session`. No explicit `namespace` parameter is accepted.

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

Routing context for `raww` SHALL be inferred from the `@<namespace>` suffix of
`target_session`. No explicit `namespace` parameter is accepted.

`raww` requests SHALL reject caller-supplied sender-like identity fields with
`validation_invalid_params`.

#### Scenario: Reject sender-like field in raww request

- **WHEN** caller submits `raww` request containing sender-like field
- **THEN** MCP rejects request with `validation_invalid_params`

## REMOVED Requirements

### Requirement: MCP Send Namespace Parameter

**Reason**: Design correction. The `namespace` parameter on `send` creates a
competing routing mechanism. Routing context for per-target operations is
inferred from each target's `@<namespace>` suffix; no explicit `namespace`
parameter is needed or accepted. Callers who previously relied on
`namespace = "GLOBAL"` to reach relay-wide targets should instead specify
fully-qualified target principal IDs (e.g. `"operator@GLOBAL"`).

**Migration**: Remove `namespace` from `send` call sites. Ensure targets carry
`@<namespace>` suffixes where cross-namespace routing is needed.
