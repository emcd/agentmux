## MODIFIED Requirements

### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools for relay MVP:

- `list.sessions`
- `send`

The legacy `list` tool is removed in this pre-MVP change.

#### Scenario: Advertise relocked list sessions tool

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `list.sessions`
- **AND** does not include legacy `list`

### Requirement: Recipient Listing Contract

`list.sessions` SHALL return bundle session listing payloads.

Single-bundle successful responses SHALL include:

- `schema_version`
- `bundle` object (`id`, `state`, `state_reason_code?`, `state_reason?`,
  `sessions[]`)

Each session entry SHALL include:

- `id`
- `name` (optional)
- `transport` (`tmux`|`acp`)

If requester identity is valid and policy denies relay-handled single-bundle
list access, MCP SHALL return `authorization_forbidden` and SHALL NOT return a
successful list payload.

#### Scenario: Deny single-bundle list sessions request with authorization_forbidden

- **WHEN** requester identity is valid
- **AND** policy denies list visibility for requester
- **THEN** MCP returns `authorization_forbidden`
- **AND** does not return successful `bundle.sessions[]` output

## ADDED Requirements

### Requirement: MCP List Sessions Selectors

`list.sessions` request parameters SHALL be:

- `bundle_name` (optional)
- `all` (optional bool; default `false`)

`bundle_name` and `all=true` SHALL be mutually exclusive.
If neither is provided, MCP SHALL resolve associated/home bundle.

#### Scenario: Reject conflicting list sessions selectors

- **WHEN** caller provides `bundle_name` and `all=true`
- **THEN** MCP rejects request with `validation_invalid_params`

### Requirement: MCP List Sessions All-Mode Aggregation

When `all=true`, MCP SHALL perform adapter-owned fanout in lexicographic
bundle-id order and return aggregate payload:

- `schema_version`
- `bundles[]` (array of canonical single-bundle `bundle` objects)

Relay all-bundle list requests are not used in MVP.

On first `authorization_forbidden` during fanout, MCP SHALL:

- stop fanout immediately,
- query no further bundles,
- return canonical non-aggregate error output.

#### Scenario: Fail fast on first authorization denial in all-mode

- **WHEN** `all=true` fanout encounters first `authorization_forbidden`
- **THEN** MCP stops fanout and returns non-aggregate error response

### Requirement: MCP List Sessions Unreachable Relay Fallback

MCP SHALL apply deterministic fallback behavior when a bundle relay is
unreachable.

When bundle relay is unreachable, MCP MAY synthesize canonical list payload only
for associated/home bundle using configuration + runtime reachability evidence.

If unreachable target is not associated/home bundle, MCP SHALL return
`relay_unavailable` and SHALL NOT synthesize cross-bundle payload.

In single-bundle mode, authorized home-bundle fallback SHALL return canonical
single-bundle payload shape.

In `all=true` mode, encountering unreachable non-home bundle SHALL fail with
`relay_unavailable` and terminate fanout.

#### Scenario: Synthesize canonical home-bundle payload on unreachable relay

- **WHEN** caller requests associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns canonical single-bundle payload with `state=down`

#### Scenario: Reject non-home unreachable fallback synthesis

- **WHEN** target bundle is not associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns `relay_unavailable`
