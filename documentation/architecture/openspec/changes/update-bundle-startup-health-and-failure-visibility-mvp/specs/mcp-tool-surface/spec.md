## MODIFIED Requirements

### Requirement: Recipient Listing Contract

`list` with `command="sessions"` SHALL return bundle session listing payloads.

Single-bundle successful responses SHALL include:

- `schema_version`
- `bundle` object:
  - `id`
  - `state` (`up`|`down`)
  - `startup_health` (`healthy`|`degraded`) (required when `state=up`;
    omitted when `state=down`)
  - `state_reason_code` (required when `state=down`; omitted when `state=up`)
  - `state_reason` (optional)
  - `startup_failure_count` (required integer)
  - `recent_startup_failures` (required array; may be empty)
  - `sessions[]`

Each session entry SHALL include:

- `id`
- `name` (optional)
- `transport` (`tmux`|`acp`)

Each `recent_startup_failures[]` entry SHALL include:

- `bundle_name`
- `session_id`
- `transport` (`tmux`|`acp`)
- `code`
- `reason`
- `timestamp`
- `sequence`
- optional `details`

If requester identity is valid and policy denies relay-handled single-bundle
list access, MCP SHALL return `authorization_forbidden` and SHALL NOT return a
successful list payload.

#### Scenario: Include startup health and startup-failure fields in successful list payload

- **WHEN** `list` with `command="sessions"` succeeds for one bundle
- **THEN** MCP response includes required startup health/state fields
- **AND** includes required startup failure history fields

#### Scenario: Omit startup health for down state

- **WHEN** bundle state is `down`
- **THEN** MCP response omits `startup_health`
- **AND** includes required `state_reason_code`

#### Scenario: Deny single-bundle list request with authorization_forbidden

- **WHEN** requester identity is valid
- **AND** policy denies list visibility for requester
- **THEN** MCP returns `authorization_forbidden`
- **AND** does not return successful `bundle.sessions[]` output

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

Home-bundle fallback startup-failure fields
(`startup_failure_count`, `recent_startup_failures`) SHALL be treated as
best-effort synthesized values from available local runtime state. When local
runtime failure history is unavailable, MCP SHALL return:

- `startup_failure_count=0`
- `recent_startup_failures=[]`

#### Scenario: Synthesize canonical home-bundle payload on unreachable relay

- **WHEN** caller requests associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns canonical single-bundle payload with `state=down`
- **AND** includes required startup failure fields

#### Scenario: Default fallback startup-failure fields when local history is unavailable

- **WHEN** home-bundle fallback is synthesized
- **AND** local runtime startup-failure history cannot be read
- **THEN** MCP returns `startup_failure_count=0`
- **AND** returns `recent_startup_failures=[]`

#### Scenario: Reject non-home unreachable fallback synthesis

- **WHEN** target bundle is not associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** MCP returns `relay_unavailable`
