## MODIFIED Requirements

### Requirement: Hello Registration Contract

Each client stream SHALL begin with a `hello` registration frame containing:

- `principal_id` (in `<id>@<namespace>` form)
- `identity_token`
- `schema_version`

`hello` SHALL carry identity and credential only. No transport class, mode, or
privilege field is accepted; relay SHALL reject unrecognized fields.

The relay process hosts a single Unix socket at `<state_root>/relay.sock` and
serves all configured bundles through that socket. The namespace portion of
`principal_id` SHALL serve as the connection type indicator:

- Session namespace (`@<bundle_name>`): relay SHALL look the bundle up in the
  bundle catalog and bind the connection to that bundle's runtime for the
  lifetime of the stream. If the bundle is not configured, relay SHALL reject
  with `validation_unknown_bundle`.
- Relay-wide namespaces (`@GLOBAL`, `@EXTERNAL`, `@RELAY`): relay SHALL NOT
  bind the connection to any bundle; the connection is relay-wide.

Credential verification and principal store lookup SHALL proceed as specified
by `add-identity-federation`.

If a second stream attempts `hello` for the same `principal_id` while the
current owner is live, relay SHALL reject the second claim with
`runtime_identity_claim_conflict`.

#### Scenario: Accept hello for session principal

- **WHEN** a client sends valid `hello` with `principal_id = "master@agentmux"`
- **AND** namespace `agentmux` maps to a configured bundle
- **AND** `master` maps to a configured bundle member
- **THEN** relay accepts hello and binds stream to bundle `agentmux`

#### Scenario: Accept hello for global user principal

- **WHEN** a client sends valid `hello` with `principal_id = "operator@GLOBAL"`
- **AND** credential is valid
- **THEN** relay accepts hello and registers connection relay-wide (no bundle binding)

#### Scenario: Reject hello for unknown bundle

- **WHEN** a client sends `hello` with `principal_id = "session@unknownbundle"`
- **AND** `unknownbundle` is not configured on the running relay
- **THEN** relay rejects with `validation_unknown_bundle`
- **AND** closes the connection without registering a stream

## ADDED Requirements

### Requirement: Request Routing Namespace

Request frames on a registered stream SHALL carry an optional `namespace` field
(formerly `bundle_name`) on the request envelope. The relay SHALL resolve the
routing context for the request as follows:

- `namespace` present, value is a bundle name → route to that bundle via
  catalog lookup, regardless of any connection binding.
- `namespace` absent + connection is bundle-bound (session principal) → route
  to the connection's bound bundle.
- `namespace` absent + connection is relay-wide (non-session principal) → relay
  SHALL return a typed error (`validation_missing_routing_namespace`).

The relay SHALL reject client-supplied `namespace` values of `"EXTERNAL"` or
`"RELAY"` with `validation_unsupported_namespace`; these are reserved for
relay-internal routing only. Routing to `"GLOBAL"` and other relay-wide
targets via target principal ID suffix inference is specified in
`add-global-namespace-routing`.

#### Scenario: Explicit bundle namespace routes to bundle

- **WHEN** a registered stream submits a request with `namespace = "agentmux"`
- **THEN** relay routes the request in the context of bundle `agentmux`
- **AND** targets are resolved against bundle `agentmux` members

#### Scenario: Absent namespace uses bound bundle

- **WHEN** a session principal stream submits a request without `namespace`
- **THEN** relay routes the request in the context of the connection's bound bundle

#### Scenario: Absent namespace on relay-wide connection returns error

- **WHEN** a relay-wide principal stream submits a request without `namespace`
- **THEN** relay returns `validation_missing_routing_namespace`

#### Scenario: EXTERNAL and RELAY namespaces are rejected

- **WHEN** a client submits a request with `namespace = "EXTERNAL"` or
  `namespace = "RELAY"`
- **THEN** relay returns `validation_unsupported_namespace`
