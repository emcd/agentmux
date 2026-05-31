## ADDED Requirements

### Requirement: Suffix-Based Target Routing

The relay SHALL infer the routing registry for each target in a `Send` request
from the `@<namespace>` suffix of the target's principal ID:

- Target with `@GLOBAL` suffix → relay-wide registry (`RegistryKey::RelayWide`)
- Target with `@<bundle>` suffix → bundle registry for `<bundle>`
- Bare target (no suffix) → sender's bound bundle registry; error if sender
  is relay-wide and has no bound bundle

The relay SHALL NOT require an explicit `namespace` field from the client to
route to relay-wide (`@GLOBAL`) targets. Clients specify targets as
fully-qualified principal IDs; the relay derives the registry from the suffix.

If a single `Send` request mixes relay-wide (`@GLOBAL`) and bundle-session
targets, the relay SHALL return `validation_conflicting_namespaces`. Cross-
namespace fan-out in one request is not supported in this slice.

Any authenticated session (bundle-bound or relay-wide) MAY send to `@GLOBAL`
targets.

#### Scenario: Bundle-bound agent sends to @GLOBAL operator

- **WHEN** a session principal sends `Send` with
  `targets = ["operator@GLOBAL"]`
- **AND** `operator@GLOBAL` is registered as a relay-wide session
- **THEN** relay delivers the message to `operator@GLOBAL`

#### Scenario: @GLOBAL principal sends to bundle session

- **WHEN** a relay-wide principal sends `Send` with
  `targets = ["agent@bundle-a"]`
- **THEN** relay routes to bundle `bundle-a` and delivers to `agent`

#### Scenario: Bare target defaults to sender's bound bundle

- **WHEN** a bundle-bound session sends `Send` with `targets = ["agent"]`
  (no `@<namespace>` suffix)
- **THEN** relay resolves `agent` within the sender's bound bundle

#### Scenario: Relay-wide sender with bare target returns error

- **WHEN** a relay-wide principal sends `Send` with a bare target (no suffix)
- **THEN** relay returns `validation_missing_routing_namespace`

#### Scenario: Mixed relay-wide and bundle targets rejected

- **WHEN** a sender includes both an `@GLOBAL` target and a `@<bundle>` target
  in the same `Send` request
- **THEN** relay returns `validation_conflicting_namespaces`

#### Scenario: Unknown @GLOBAL target

- **WHEN** a sender targets a principal ID with `@GLOBAL` suffix that is not
  registered in the relay-wide registry
- **THEN** relay returns `validation_unknown_target`

### Requirement: GLOBAL Namespace List

The relay SHALL return the set of currently registered relay-wide sessions
when `List` is requested with `namespace = "GLOBAL"`.

#### Scenario: List relay-wide sessions

- **WHEN** a principal sends `List` with `namespace = "GLOBAL"`
- **AND** one or more relay-wide sessions are currently registered
- **THEN** relay returns `RelayResponse::List` containing those sessions

#### Scenario: List with no relay-wide sessions registered

- **WHEN** a principal sends `List` with `namespace = "GLOBAL"`
- **AND** no relay-wide sessions are currently registered
- **THEN** relay returns `RelayResponse::List` with an empty session set

### Requirement: Retire GLOBAL Routing Stub

The relay SHALL NOT return `validation_namespace_routing_unavailable`. This
temporary error code is retired when suffix-based GLOBAL routing is
implemented.

#### Scenario: @GLOBAL target no longer returns stub error

- **WHEN** a session sends `Send` with an `@GLOBAL` target
- **THEN** the relay SHALL NOT return `validation_namespace_routing_unavailable`
- **AND** SHALL route or return an appropriate typed error per the suffix
  routing rules above
