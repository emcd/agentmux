## MODIFIED Requirements

### Requirement: Suffix-Based Target Routing

The relay SHALL infer the routing registry for each target in a `Send` request
from the `@<namespace>` suffix of the target's principal ID:

- Target with `@GLOBAL` suffix → relay-wide registry (`RegistryKey::RelayWide`)
- Target with `@<bundle>` suffix → bundle registry for `<bundle>`
- Target with `@EXTERNAL` or `@RELAY` suffix → `validation_unsupported_namespace`
- Bare target (no suffix) → sender's bound bundle registry; error if sender
  is relay-wide and has no bound bundle

The relay SHALL NOT require an explicit `namespace` field from the client to
route to relay-wide (`@GLOBAL`) or cross-bundle (`@<bundle>`) targets. Clients
specify targets as fully-qualified principal IDs; the relay derives the registry
from the suffix.

A single `Send` request MAY mix relay-wide (`@GLOBAL`) and bundle-session
targets. The relay SHALL validate all targets before any delivery and SHALL fan
out delivery to each target in its respective namespace independently.

Any authenticated session (bundle-bound or relay-wide) MAY send to `@GLOBAL`
targets or to `@<bundle>` targets in any known bundle.

#### Scenario: Bundle-bound agent sends to @GLOBAL operator

- **WHEN** a session principal sends `Send` with
  `targets = ["operator@GLOBAL"]`
- **AND** `operator@GLOBAL` is registered as a relay-wide session
- **THEN** relay delivers the message to `operator@GLOBAL`

#### Scenario: @GLOBAL principal sends to bundle session

- **WHEN** a relay-wide principal sends `Send` with
  `targets = ["agent@bundle-a"]`
- **THEN** relay routes to bundle `bundle-a` and delivers to `agent`

#### Scenario: Send fans out across multiple namespaces

- **WHEN** a sender includes targets from different namespaces in one `Send`
  (e.g., `["agent@bundle-b", "operator@GLOBAL"]`)
- **AND** all targets are registered in their respective namespaces
- **THEN** relay delivers the message to each target independently and returns
  per-target results in `RelayResponse::Send`

#### Scenario: Bare target defaults to sender's bound bundle

- **WHEN** a bundle-bound session sends `Send` with `targets = ["agent"]`
  (no `@<namespace>` suffix)
- **THEN** relay resolves `agent` within the sender's bound bundle

#### Scenario: Relay-wide sender with bare target returns error

- **WHEN** a relay-wide principal sends `Send` with a bare target (no suffix)
- **THEN** relay returns `validation_missing_routing_namespace`

#### Scenario: Unknown @GLOBAL target

- **WHEN** a sender targets a principal ID with `@GLOBAL` suffix that is not
  registered in the relay-wide registry
- **THEN** relay returns `validation_unknown_target`

#### Scenario: Unknown @<bundle> target

- **WHEN** a sender targets a principal ID with `@<bundle>` suffix where
  `<bundle>` is not a configured bundle, or the bare session ID is not a
  member of that bundle
- **THEN** relay returns `validation_unknown_target`

#### Scenario: @EXTERNAL or @RELAY target rejected

- **WHEN** a sender includes a target with `@EXTERNAL` or `@RELAY` suffix
- **THEN** relay returns `validation_unsupported_namespace`
