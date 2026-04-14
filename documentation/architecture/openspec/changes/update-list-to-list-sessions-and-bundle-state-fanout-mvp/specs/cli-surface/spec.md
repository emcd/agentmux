## MODIFIED Requirements

### Requirement: List Command Authorization Semantics

CLI `list sessions` SHALL map to capability `list.read` for relay-handled
single-bundle requests.
If requester identity is valid and policy denies list access, CLI SHALL surface
`authorization_forbidden` and SHALL NOT render a successful session list.

#### Scenario: Return authorization denial for single-bundle list sessions request

- **WHEN** operator invokes `agentmux list sessions`
- **AND** policy denies list visibility for resolved requester identity
- **THEN** CLI returns `authorization_forbidden`
- **AND** does not present successful `bundle.sessions[]` output

## ADDED Requirements

### Requirement: List Sessions Command Surface

The CLI SHALL expose session-listing surfaces:

- `agentmux list sessions --bundle <bundle-id>`
- `agentmux list sessions --all`

`--bundle` and `--all` SHALL be mutually exclusive.
If neither selector is provided, CLI SHALL resolve associated/home bundle.

The legacy `agentmux list` surface is removed in this pre-MVP change.

#### Scenario: Reject conflicting list sessions selectors

- **WHEN** operator provides `--bundle` and `--all` together
- **THEN** CLI rejects invocation with `validation_invalid_params`

#### Scenario: Resolve home bundle when selector is omitted

- **WHEN** operator invokes `agentmux list sessions` with no selector
- **THEN** CLI targets associated/home bundle

### Requirement: List Sessions Machine Output Contract

CLI machine-readable successful output for single-bundle mode SHALL include:

- `schema_version`
- `bundle` object:
  - `id`
  - `state` (`up`|`down`)
  - `state_reason_code` (required when `state=down`)
  - `state_reason` (optional)
  - `sessions[]` with `id`, `name?`, `transport`

For `--all` mode, CLI machine output SHALL include:

- `schema_version`
- `bundles[]` (array of canonical single-bundle `bundle` objects)

`bundles[]` ordering SHALL be lexicographic by bundle id.

#### Scenario: Return lexicographically ordered all-mode output

- **WHEN** operator invokes `agentmux list sessions --all`
- **THEN** CLI output contains `bundles[]` ordered lexicographically by
  `bundle.id`

### Requirement: List Sessions Fanout Behavior

In `--all` mode, CLI SHALL perform adapter-owned fanout by querying bundles in
lexicographic order.
Relay all-bundle list requests are not used in MVP.

On first `authorization_forbidden` from a bundle query, CLI SHALL:

- stop fanout immediately,
- query no further bundles,
- return canonical non-aggregate error output.

#### Scenario: Fail fast on first all-mode authorization denial

- **WHEN** `--all` fanout encounters first `authorization_forbidden`
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

In `--all` mode, encountering unreachable non-home bundle SHALL fail with
`relay_unavailable` and terminate fanout.

#### Scenario: Synthesize canonical home-bundle payload when relay is unreachable

- **WHEN** operator requests associated/home bundle session listing
- **AND** bundle relay is unreachable
- **THEN** CLI returns canonical single-bundle payload with `state=down`

#### Scenario: Reject non-home unreachable fallback synthesis

- **WHEN** target bundle is not associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** CLI returns `relay_unavailable`
