## ADDED Requirements

### Requirement: MCP Meta-Tool Command Enum Schemas

MCP meta-tool input schemas SHALL expose enum constraints for their top-level
`command` fields:

- `list.command`: `principals`, `namespaces`, `relays`, `decisions`
- `updown.command`: `up`, `down`
- `new.command`: `peer`
- `change.command`: `psk`

The constraints SHALL render as flat string enums in advertised MCP tool input
schemas. Missing commands, unsupported values, and unknown fields SHALL continue
to use the existing validation taxonomy.

#### Scenario: Advertise list command enum

- **WHEN** an MCP client enumerates tools
- **THEN** the `list.command` schema is a string enum containing `principals`,
  `namespaces`, `relays`, and `decisions`

#### Scenario: Advertise administrative command enums

- **WHEN** an MCP client enumerates tools
- **THEN** `updown.command` is constrained to `up` and `down`
- **AND** `new.command` is constrained to `peer`
- **AND** `change.command` is constrained to `psk`

#### Scenario: Preserve unknown command rejection

- **WHEN** a caller supplies a command outside the advertised enum
- **THEN** MCP rejects it with the existing invalid-params taxonomy

### Requirement: MCP Relay Discovery

MCP `list` with `command="relays"` SHALL enumerate the local relay's configured
outbound peer aliases. Its command-scoped `args` object SHALL accept no fields.

Successful response SHALL contain `schema_version` and `relays[]`. Each relay
entry SHALL contain only `alias`. Entries SHALL be sorted lexicographically.
Listing SHALL NOT dial peers or expose addresses, `connect-as` identities,
credential paths, or credentials.

#### Scenario: List configured relay aliases

- **WHEN** a caller invokes `list` with `command="relays"`
- **THEN** MCP returns sorted `relays[]` entries containing local peer aliases
- **AND** opens no peer connection

#### Scenario: List no configured relays

- **WHEN** the local relay has no configured `[[peers]]`
- **THEN** `list.relays` succeeds with `relays=[]`

### Requirement: MCP Namespace Discovery

MCP `list` with `command="namespaces"` SHALL enumerate namespaces on the local
relay or on one configured foreign relay.

Command-scoped arguments SHALL contain optional `relay`, the origin-local
`[[peers]].alias`. When omitted, discovery is local. Successful responses SHALL
contain `schema_version` and sorted unique `namespaces[]`; foreign responses SHALL
also contain the selected `relay`, while local responses SHALL omit it.
An empty/whitespace or `"*"` relay selector SHALL be rejected with
`validation_invalid_params`.

Local results SHALL follow the requester's `list` authorization. Foreign results
SHALL contain only namespaces returned by the authenticated peer relay after its
ingress filtering.

#### Scenario: Discover local namespaces

- **WHEN** a caller invokes `list.namespaces` without `relay`
- **THEN** MCP returns locally visible namespace identifiers
- **AND** omits `relay` from the response

#### Scenario: Discover foreign namespaces

- **WHEN** a caller invokes `list.namespaces` with `relay="west"`
- **THEN** MCP returns namespace identifiers authored by peer `west`
- **AND** returns `relay="west"`

#### Scenario: Foreign namespace discovery denied by peer

- **WHEN** peer `west` has no ingress scope permitting discovery
- **THEN** MCP propagates `authorization_forbidden`

#### Scenario: Reject malformed namespace relay selector

- **WHEN** a caller invokes `list.namespaces` with an empty/whitespace or `"*"`
  relay selector
- **THEN** MCP rejects the request with `validation_invalid_params`

### Requirement: MCP Cross-Relay Principal Discovery

`list` with `command="principals"` SHALL accept optional `args.relay` in addition
to `args.namespace`.

When `relay` is absent, existing local namespace behavior SHALL remain unchanged.
When `relay` is present, `namespace` SHALL be required and SHALL name one concrete
namespace; omitted namespace, `"*"`, and unsupported reserved namespace tokens
SHALL be rejected with `validation_invalid_params` before relay submission.

Foreign success responses SHALL contain `schema_version`, the selected `relay`,
and `bundles[]`. Each bundle SHALL be peer-authored, retain its foreign id, and
use the canonical listed bundle shape. Scope-filtered subsets SHALL carry
`principals_partial=true`; complete listings SHALL omit that field. MCP SHALL NOT
synthesize peer contents from local configuration.

#### Scenario: Discover principals in a foreign namespace

- **WHEN** a caller invokes `list.principals` with `relay="west"` and
  `namespace="myapp"`
- **THEN** MCP returns peer-authored principals from namespace `myapp`
- **AND** returns `relay="west"`

#### Scenario: Reject foreign principals without concrete namespace

- **WHEN** a caller supplies `relay` with omitted namespace or namespace `"*"`
- **THEN** MCP rejects the request with `validation_invalid_params`
- **AND** does not submit it to the relay

#### Scenario: Preserve peer authorization denial

- **WHEN** the receiving peer denies principal discovery
- **THEN** MCP propagates `authorization_forbidden`

#### Scenario: Preserve partial visibility marker

- **WHEN** a peer returns a principal-scoped subset of a namespace
- **THEN** MCP preserves `principals_partial=true` on that bundle

## MODIFIED Requirements

### Requirement: MCP Help Tool

The system SHALL expose a read-only MCP tool named `help` that returns
tool/command discovery metadata and JSON argument schemas.

`help` SHALL support query modes:

- no query (or `query="agentmux"`) returns namespace-level tool inventory
- `query="list"` returns list meta-tool command catalog
- `query="list.principals"` returns exact `list` command argument schema and
  invoke shape, including optional `namespace` and `relay`
- `query="list.namespaces"` returns exact `list` command argument schema and
  invoke shape, including optional `relay`
- `query="list.relays"` returns exact empty argument schema and invoke shape
- `query="list.decisions"` returns exact `list` command argument schema and
  invoke shape for listing pending decisions
- `query="send"` or `query="look"` or `query="raww"` returns exact tool
  argument schemas and invoke shapes
- `query="choose"` returns the `choose` tool argument schema and invoke shape

Unknown help queries SHALL fail fast with `validation_invalid_params`.

#### Scenario: Return namespace inventory with no query

- **WHEN** an MCP client calls `help` without `query`
- **THEN** the response includes namespace-level tool inventory
- **AND** includes `list`, `help`, `look`, `send`, `raww`, and `choose`

#### Scenario: Return list meta-tool command catalog

- **WHEN** an MCP client calls `help` with `query="list"`
- **THEN** the response lists supported `list` commands
- **AND** includes `list.principals`
- **AND** includes `list.namespaces`
- **AND** includes `list.relays`
- **AND** includes `list.decisions`

#### Scenario: Return list.principals argument schema

- **WHEN** an MCP client calls `help` with `query="list.principals"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** the schema includes optional string fields `namespace` and `relay`
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="principals"`

#### Scenario: Return list.namespaces argument schema

- **WHEN** an MCP client calls `help` with `query="list.namespaces"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** the schema includes optional string field `relay`
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="namespaces"`

#### Scenario: Return list.relays argument schema

- **WHEN** an MCP client calls `help` with `query="list.relays"`
- **THEN** the command-scoped `args` schema contains no properties
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="relays"`

#### Scenario: Return list.decisions argument schema

- **WHEN** an MCP client calls `help` with `query="list.decisions"`
- **THEN** the response includes JSON schema for command-scoped `args`
- **AND** includes canonical invoke shape with top-level tool `list`
- **AND** includes `command="decisions"`

#### Scenario: Return choose tool argument schema

- **WHEN** an MCP client calls `help` with `query="choose"`
- **THEN** the response includes JSON argument schema for the `choose` tool
- **AND** includes canonical invoke shape

### Requirement: MCP List Sessions Selectors

`list` request parameters for principals listing SHALL be:

- `command` (required, equal to `"principals"`)
- `args` (optional object)
  - `namespace` (optional string)
  - `relay` (optional string; switches to foreign semantics when present)

When `relay` is absent, `namespace` SHALL retain its existing meanings:

- omitted or null selects the associated/home bundle (default)
- a bundle name selects that specific bundle
- `"GLOBAL"` selects relay-wide principals
- `"*"` performs adapter-owned fan-out across all local namespaces

`"*"` SHALL be the only fan-out token; no `"ALL"` alias SHALL be accepted.
`"EXTERNAL"` and `"RELAY"` SHALL NOT be valid `list` selectors. The prior
`bundle_name` and `all` selectors remain removed with no compatibility alias.

When `relay` is present, `namespace` SHALL be required and SHALL name one
concrete namespace. Omitted namespace and `"*"` SHALL be rejected with
`validation_invalid_params`. Empty/whitespace namespace and empty/whitespace or
`"*"` relay selectors SHALL also be rejected with `validation_invalid_params`.

#### Scenario: Reject missing or unsupported list command

- **WHEN** caller omits `command` or provides an unsupported list command
- **THEN** MCP rejects request with `validation_invalid_params`

#### Scenario: Resolve home bundle when namespace omitted

- **WHEN** caller omits both `relay` and `namespace`
- **THEN** MCP resolves the associated/home bundle

#### Scenario: Select relay-wide principals with GLOBAL namespace

- **WHEN** caller provides no `relay` and `namespace="GLOBAL"`
- **THEN** MCP lists relay-wide principals

#### Scenario: Fan out across all namespaces with star token

- **WHEN** caller supplies no `relay` and `namespace="*"`
- **THEN** MCP performs adapter-owned fan-out across all local namespaces

#### Scenario: Reject ALL alias as fan-out token

- **WHEN** caller provides `namespace="ALL"`
- **THEN** MCP rejects request with `validation_invalid_params`

#### Scenario: Relay selector activates foreign validation

- **WHEN** caller supplies `relay`
- **THEN** MCP requires one concrete foreign namespace

#### Scenario: Reject malformed foreign selectors

- **WHEN** caller supplies an empty/whitespace or `"*"` relay selector
- **OR** supplies an empty/whitespace foreign namespace
- **THEN** MCP rejects request with `validation_invalid_params`
