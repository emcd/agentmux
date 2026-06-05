## MODIFIED Requirements

### Requirement: MCP List Sessions Selectors

`list` request parameters for principals listing SHALL be:

- `command` (required, must equal `"principals"`)
- `args` (optional object)
  - `namespace` (optional string)

`namespace` SHALL select the listing scope:

- omitted or null → associated/home bundle (default)
- a bundle name → that specific bundle
- `"GLOBAL"` → relay-wide principals
- `"*"` → adapter-owned fan-out across all namespaces

`"*"` SHALL be the only fan-out token; no `"ALL"` alias SHALL be accepted.
`"EXTERNAL"` and `"RELAY"` SHALL NOT be valid `list` selectors. The prior
`bundle_name` and `all` selectors are removed with no compatibility alias.

#### Scenario: Reject missing or unsupported list command

- **WHEN** caller omits `command` or provides a value other than `"principals"`
- **THEN** MCP rejects request with `validation_invalid_params`

#### Scenario: Resolve home bundle when namespace omitted

- **WHEN** caller omits `namespace`
- **THEN** MCP resolves the associated/home bundle

#### Scenario: Select relay-wide principals with GLOBAL namespace

- **WHEN** caller provides `namespace="GLOBAL"`
- **THEN** MCP lists relay-wide principals

#### Scenario: Fan out across all namespaces with star token

- **WHEN** caller provides `namespace="*"`
- **THEN** MCP performs adapter-owned fan-out across all namespaces

#### Scenario: Reject ALL alias as fan-out token

- **WHEN** caller provides `namespace="ALL"`
- **THEN** MCP rejects request with `validation_invalid_params`

### Requirement: MCP List Sessions All-Mode Aggregation

When `list` is called with `command="principals"` and `namespace="*"`, MCP SHALL
perform adapter-owned fanout in lexicographic bundle-id order and return aggregate
payload:

- `schema_version`
- `bundles[]` (array of canonical single-bundle `bundle` objects)

Relay all-bundle list requests are not used; the relay accepts only a single
resolved namespace (a bundle name or `GLOBAL`) and never receives `"*"`.

On first `authorization_forbidden` during fanout, MCP SHALL:

- stop fanout immediately,
- query no further bundles,
- return canonical non-aggregate error output.

#### Scenario: Fail fast on first authorization denial in all-mode

- **WHEN** `namespace="*"` fanout encounters first `authorization_forbidden`
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

In `namespace="*"` mode, encountering unreachable non-home bundle SHALL fail with
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

### Requirement: Recipient Listing Contract

`list` with `command="principals"` SHALL return bundle principal listing
payloads.

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
  - `principals[]`

Each `principals[]` entry SHALL include:

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

- **WHEN** `list` with `command="principals"` succeeds for one bundle
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
- **AND** does not return successful `bundle.principals[]` output

### Requirement: MCP Help Tool

The system SHALL expose a read-only MCP tool named `help` that returns
tool/command discovery metadata and JSON argument schemas.

`help` SHALL support query modes:

- no query (or `query="agentmux"`) returns namespace-level tool inventory
- `query="list"` returns list meta-tool command catalog
- `query="list.principals"` returns exact `list` command argument schema and
  invoke shape
- `query="send"` or `query="look"` or `query="raww"` returns exact tool
  argument schemas and invoke shapes
- `query="grant"` returns grant meta-tool command catalog
- `query="grant.list"` returns exact `grant` command argument
  schema and invoke shape for listing pending requests
- `query="grant.resolve"` returns exact `grant` command argument
  schema and invoke shape for submitting decisions

Unknown help queries SHALL fail fast with `validation_invalid_params`.

#### Scenario: Return namespace inventory with no query

- **WHEN** an MCP client calls `help` without `query`
- **THEN** the response includes namespace-level tool inventory
- **AND** includes `list`, `help`, `look`, `send`, `raww`, and `grant`

#### Scenario: Return list meta-tool command catalog

- **WHEN** an MCP client calls `help` with `query="list"`
- **THEN** the response lists supported `list` commands
- **AND** includes `list.principals`

#### Scenario: Return list.principals argument schema

- **WHEN** an MCP client calls `help` with `query="list.principals"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="principals"`

#### Scenario: Return grant meta-tool command catalog

- **WHEN** an MCP client calls `help` with `query="grant"`
- **THEN** the response lists supported `grant` commands
- **AND** includes `grant.list`
- **AND** includes `grant.resolve`

#### Scenario: Return grant.list argument schema

- **WHEN** an MCP client calls `help` with `query="grant.list"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `grant`
- **AND** includes `command="list"`
