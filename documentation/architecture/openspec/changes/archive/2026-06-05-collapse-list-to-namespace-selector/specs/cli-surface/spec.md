## MODIFIED Requirements

### Requirement: List Sessions Command Surface

The CLI SHALL expose a principals-listing surface:

- `agentmux list principals --namespace <namespace>`

`--namespace` SHALL select the listing scope:

- omitted → associated/home bundle (default)
- a bundle name → that specific bundle
- `GLOBAL` → relay-wide principals
- `*` → adapter-owned fan-out across all namespaces

The prior `--bundle` and `--all` flags are removed with no compatibility alias.
The legacy `agentmux list` surface remains removed.

#### Scenario: Resolve home bundle when namespace is omitted

- **WHEN** operator invokes `agentmux list principals` with no selector
- **THEN** CLI targets associated/home bundle

#### Scenario: Fan out across all namespaces with star token

- **WHEN** operator invokes `agentmux list principals --namespace '*'`
- **THEN** CLI performs adapter-owned fan-out across all namespaces

### Requirement: List Sessions Machine Output Contract

CLI machine-readable successful output for single-bundle mode SHALL include:

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
  - `principals[]` with `id`, `name?`, `transport`

Each `recent_startup_failures[]` entry SHALL include:

- `bundle_name`
- `session_id`
- `transport` (`tmux`|`acp`)
- `code`
- `reason`
- `timestamp`
- `sequence`
- optional `details`

For `--namespace '*'` mode, CLI machine output SHALL include:

- `schema_version`
- `bundles[]` (array of canonical single-bundle `bundle` objects)

`bundles[]` ordering SHALL be lexicographic by bundle id.

#### Scenario: Return startup health and startup-failure fields in single-bundle output

- **WHEN** operator invokes `agentmux list principals --namespace <bundle-id>`
- **THEN** CLI output includes required startup health/state fields
- **AND** includes required startup failure history fields

#### Scenario: Return lexicographically ordered all-mode output

- **WHEN** operator invokes `agentmux list principals --namespace '*'`
- **THEN** CLI output contains `bundles[]` ordered lexicographically by
  `bundle.id`

### Requirement: List Sessions Fanout Behavior

In `--namespace '*'` mode, CLI SHALL perform adapter-owned fanout by querying
bundles in lexicographic order.
Relay all-bundle list requests are not used.

On first `authorization_forbidden` from a bundle query, CLI SHALL:

- stop fanout immediately,
- query no further bundles,
- return canonical non-aggregate error output.

#### Scenario: Fail fast on first all-mode authorization denial

- **WHEN** `--namespace '*'` fanout encounters first `authorization_forbidden`
- **THEN** CLI stops fanout
- **AND** does not return partial aggregate success payload

### Requirement: List Sessions Unreachable Relay Fallback

CLI SHALL apply deterministic fallback behavior when a bundle relay is
unreachable.

When bundle relay is unreachable, CLI MAY synthesize canonical list payload only
for associated/home bundle using configuration + runtime reachability evidence.

If unreachable target is not associated/home bundle, CLI SHALL return
`relay_unavailable` and SHALL NOT synthesize cross-bundle payload.

In single-bundle mode, authorized home-bundle fallback SHALL return canonical
single-bundle payload shape (not raw transport passthrough).

In `--namespace '*'` mode, encountering unreachable non-home bundle SHALL fail
with `relay_unavailable` and terminate fanout.

Home-bundle fallback startup-failure fields
(`startup_failure_count`, `recent_startup_failures`) SHALL be treated as
best-effort synthesized values from available local runtime state. When local
runtime failure history is unavailable, CLI SHALL return:

- `startup_failure_count=0`
- `recent_startup_failures=[]`

#### Scenario: Synthesize canonical home-bundle payload when relay is unreachable

- **WHEN** operator requests associated/home bundle principal listing
- **AND** bundle relay is unreachable
- **THEN** CLI returns canonical single-bundle payload with `state=down`
- **AND** includes required startup failure fields

#### Scenario: Default fallback startup-failure fields when local history is unavailable

- **WHEN** home-bundle fallback is synthesized
- **AND** local runtime startup-failure history cannot be read
- **THEN** CLI returns `startup_failure_count=0`
- **AND** returns `recent_startup_failures=[]`

#### Scenario: Reject non-home unreachable fallback synthesis

- **WHEN** target bundle is not associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** CLI returns `relay_unavailable`
