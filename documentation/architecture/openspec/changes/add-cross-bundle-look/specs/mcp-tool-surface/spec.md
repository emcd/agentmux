## MODIFIED Requirements

### Requirement: MCP Look Tool

The system SHALL expose a read-only MCP inspection tool named `look`.

`look` SHALL support:

- `target_session` (required session identifier; MAY be a peer-qualified
  `<session>@<bundle>` id to inspect a session in a peer bundle)
- `lines` (optional positive integer)
- `bundle_name` (optional, redundant; does not select or reject a peer bundle)

The tool forwards `target_session` to the relay verbatim; cross-bundle
resolution and authorization are performed by the relay and surfaced unchanged.

#### Scenario: Advertise look tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `look`

#### Scenario: Reject invalid lines in look request

- **WHEN** a caller provides `lines` outside valid range
- **THEN** the tool returns `validation_invalid_lines`

#### Scenario: Inspect peer bundle session via qualified target

- **WHEN** a caller provides `target_session = "<session>@<peer-bundle>"`
  naming a bundle other than the associated bundle
- **THEN** the tool forwards the target and returns the relay's peer-bundle
  snapshot when the requester is authorized at `look = all:all`

#### Scenario: Reject unknown bundle

- **WHEN** the target names a bundle that is not configured on the relay
- **THEN** the tool returns `validation_unknown_bundle`

#### Scenario: Reject unknown target

- **WHEN** caller requests inspection for a session that is not a member of the
  resolved bundle
- **THEN** tool returns `validation_unknown_target`
