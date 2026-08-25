## MODIFIED Requirements

### Requirement: MCP Tool Set

The system SHALL expose the following MCP tools:

- `list`
- `help`
- `look`
- `send`
- `raww`
- `choose`
- `updown`
- `new`
- `change`
- `drop`

The relocked pre-stable MCP surface uses `list.principals` with no
compatibility alias for the prior `list.sessions` shape.

#### Scenario: Advertise relocked list meta-tool

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `list`
- **AND** includes `help`
- **AND** includes `look`
- **AND** includes `send`
- **AND** includes `raww`
- **AND** includes `choose`
- **AND** does not include `list.sessions`
- **AND** does not include `grant`

#### Scenario: Advertise admin meta-tools

- **WHEN** an MCP client enumerates available tools
- **THEN** tool inventory includes `updown`
- **AND** includes `new`
- **AND** includes `change`
- **AND** includes `drop`

### Requirement: Error Object Contract

Tool failures SHALL return a structured error object with:

- `code`
- `message`
- `details` (optional object)

The system SHALL use stable machine-readable error codes.

For `authorization_forbidden`, `details` SHALL include:

- required:
  - `capability`
  - `requester_session`
  - `bundle_name`
  - `reason`
- optional:
  - `target_session`
  - `targets`
  - `policy_rule_id`

A validation failure that is decidable without privileged state SHALL be
returned before authorization denials. This covers request schema and field
format, mutually exclusive options, association state, and any check resolvable
from the request together with the requester's own authenticated identity.

A validation that is decidable only by reading a store or resource the
authorization denial protects SHALL follow authorization instead. Returning such
a failure to an unauthorized caller would disclose whether the named resource
exists, which is precisely what the denial withholds; an unauthorized caller
SHALL receive `authorization_forbidden` whether or not the referenced resource
exists. Where a tool's own requirement names which of its checks are
store-backed, that classification governs for that tool.

Every relay-backed tool (`list` principals/namespaces/relays/decisions, `send`,
`look`, `raww`, `choose`, `updown`, `new`, `change`, `drop`) requires the MCP
server to be associated with a bundle and session. A call on an unassociated
server (one that holds no relay stream) SHALL be rejected before any relay
contact with a validation-shaped error coded `validation_unassociated_server`,
whose `details` SHALL carry a canonical `reason` of `unassociated_server` and a
`remedy` naming the command that starts an associated server. `help` is the sole
association-independent tool and SHALL remain callable regardless of
association.

#### Scenario: Reject relay-backed tool on unassociated server

- **WHEN** any relay-backed tool is invoked on an MCP server with no
  bundle+session association
- **THEN** the tool returns error code `validation_unassociated_server`
- **AND** `details` include `reason = "unassociated_server"` and a `remedy`
  string

#### Scenario: Unknown bundle error

- **WHEN** a caller references a bundle that does not exist
- **THEN** the tool returns error code `validation_unknown_bundle`
- **AND** includes a human-readable message

#### Scenario: Unknown target error

- **WHEN** `send` targets a token that is not a canonical send target identifier
- **THEN** the tool returns error code `validation_unknown_target`
- **AND** includes a human-readable message

#### Scenario: Return canonical authorization denial schema

- **WHEN** request is valid/resolved but denied by policy
- **THEN** the tool returns `authorization_forbidden`
- **AND** details include the required denial fields

#### Scenario: Locally decidable validation precedes an authorization denial

- **WHEN** an unauthorized caller submits a request that also fails a validation
  decidable from the request and the caller's own identity
- **THEN** the tool returns the validation error rather than
  `authorization_forbidden`

#### Scenario: Store-backed existence check follows authorization

- **WHEN** an unauthorized caller references a resource whose existence is
  readable only from a store the denial protects
- **THEN** the tool returns `authorization_forbidden`
- **AND** does not reveal whether the referenced resource exists

### Requirement: MCP New Tool

The system SHALL expose a meta-tool `new` that registers a principal credential.
`new` SHALL require `command="peer"`.

`new peer` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)
- `scope` (optional)
- `output_path` (optional, absolute path)
- `write_to_config` (optional, boolean)

`output_path` and `write_to_config` SHALL be mutually exclusive; a request
supplying both SHALL be rejected with `validation_invalid_params` before any
relay request is issued.

An ingress `scope` names either an exact `<id>@<namespace>` principal or a bare
namespace; it is not a policy tier. Where `scope` is one of the policy-tier
values `none`, `self`, `home`, or `all`, the response SHALL carry a diagnostic
with the code `advisory_scope_resembles_policy_tier`, and the relay SHALL
register the principal anyway. The scope values are legal namespace names, so
the diagnostic SHALL NOT fail the request, and a `scope` that merely resolves to
nothing SHALL NOT produce it.

Diagnostics SHALL travel in the response payload rather than on any process
output stream, because the relay is a separate process from both callers and its
own stderr reaches neither. Each diagnostic SHALL carry a `code` and a
human-readable `message`. The MCP tool SHALL preserve the diagnostics in its
structured result; the CLI SHALL render each diagnostic to its own stderr and
SHALL still exit zero.

The relay SHALL generate the PSK, persist only its SHA-256 hash, and route the
raw value to one of three credential destinations:

- **Response** (default, when neither option is set): the relay SHALL return the
  raw PSK once in the response.
- **Path** (`output_path` set): the relay SHALL write the PSK to that path and
  omit it from the response. The path MUST be absolute, its parent MUST already
  exist, and the target MUST NOT be a symlink; a path failing these
  preconditions SHALL be rejected with `validation_invalid_output_path`.
- **Config** (`write_to_config` set): the relay SHALL write the PSK to the
  principal's relay-owned canonical credential path and omit it from the
  response. Config is derivable only for **session** principals, whose
  credential location the relay owns; the relay SHALL reject Config for relay,
  user, or application principals with
  `validation_config_destination_unsupported`. Before deriving any path the
  relay SHALL reject a `principal_id` whose components are not a valid session
  identity — the configured session-id grammar for the id and the canonical
  bundle-name grammar for the namespace (which permits dotted names but rejects
  the traversal-only `.`/`..` segments and path separators) — with
  `validation_invalid_principal_id`.

The relay SHALL validate and stage the selected destination before mutating the
principal store; a rejected or failed destination SHALL NOT register the
principal. `new` is a relay-wide operation: the relay SHALL authorize the
connection principal against an `all`-scoped `new.peer` grant, and a
bundle-relative `home` grant SHALL be insufficient.

#### Scenario: Advertise new tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `new`

#### Scenario: Mint PSK for a new peer principal

- **WHEN** a caller invokes `new` with `command="peer"` and a `principal_id`
- **THEN** the relay registers the principal and returns the minted PSK
- **AND** omits the raw PSK from the response when `output_path` or
  `write_to_config` selected a file destination

#### Scenario: Warn on a scope drawn from the policy-tier vocabulary

- **WHEN** a caller invokes `new` with `command="peer"` and `scope` set to
  `none`, `self`, `home`, or `all`
- **THEN** the response carries an `advisory_scope_resembles_policy_tier`
  diagnostic
- **AND** registers the principal with that scope
- **AND** the request succeeds

#### Scenario: MCP caller receives the scope diagnostic

- **WHEN** an MCP caller invokes `new` with a policy-tier `scope`
- **THEN** the structured result carries the diagnostic with its code and
  message

#### Scenario: CLI renders the scope diagnostic to stderr

- **WHEN** an operator runs `new peer` with a policy-tier `--scope`
- **THEN** the CLI writes the diagnostic message to its own stderr
- **AND** exits zero

#### Scenario: Register an unresolvable scope without a diagnostic

- **WHEN** a caller invokes `new` with `command="peer"` and a `scope` naming a
  namespace that does not exist on this relay
- **THEN** the relay registers the principal without emitting the
  vocabulary-collision diagnostic

#### Scenario: Reject config destination for a non-session principal

- **WHEN** a caller invokes `new` with `write_to_config` for a principal whose
  type is not session
- **THEN** the relay returns `validation_config_destination_unsupported`
- **AND** does not register the principal

#### Scenario: Reject mutually exclusive credential destinations

- **WHEN** a caller supplies both `output_path` and `write_to_config`
- **THEN** the request is rejected with `validation_invalid_params`
- **AND** no relay request is issued

### Requirement: MCP New Success Payload Contract

Successful `new` responses SHALL include:

- `schema_version`
- `principal_id`
- `principal_type`
- `config_snippet`
- `psk` (present only when the credential destination was Response)
- `written_path` (present only when the PSK was written to a file — the caller
  path for a Path destination, or the relay-owned canonical path for Config)
- `diagnostics` (present only when the request produced one or more advisories;
  each entry carries a `code` and a `message`)

#### Scenario: Return minted credential payload for new peer

- **WHEN** a `new` `command="peer"` request succeeds
- **THEN** the response includes the required credential fields

#### Scenario: Omit diagnostics when the request raised none

- **WHEN** a `new` `command="peer"` request succeeds without raising an advisory
- **THEN** the response carries no `diagnostics` entries

## ADDED Requirements

### Requirement: MCP Drop Tool

The system SHALL expose a meta-tool `drop` that deletes a principal from the
relay-wide principal store. `drop` SHALL require `command="peer"`.

`drop peer` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)

The relay SHALL reject dropping the authenticated requester's own principal
with `validation_self_drop_forbidden` and SHALL mutate nothing. Dropping a
principal revokes every session bound to it, which for a self-drop
includes the connection carrying the request, so the requester would lose the
response to an operation that had already committed and could not tell a
committed drop from a failed one. An operator dropping their own principal
SHALL do so from a different authenticated principal.

This check is decidable without privileged state and SHALL therefore precede
authorization under the Error Object Contract: it compares the requested
`principal_id` against the requester's own authenticated identity and needs
neither the principal store nor the requester's grants. A caller dropping their
own id SHALL receive `validation_self_drop_forbidden` deterministically,
including when they hold no `drop.peer` grant.

The relay SHALL reject a `principal_id` that is not registered with
`validation_unknown_principal`, and SHALL NOT treat dropping an absent
principal as success. This check is store-backed and SHALL therefore follow
authorization under the Error Object Contract, so an unauthorized caller
receives `authorization_forbidden` whether or not the principal exists.

Dropping a principal SHALL delete its store record and then apply the revocation
behavior specified for an explicitly revoked principal: every relay session
bound to that principal is torn down after a `runtime_identity_revoked` typed
error frame, and trusted-host streams whose scope covers the principal receive
an `identity.revoked` event. The relay SHALL persist the store before revoking
anything; a failed persist SHALL revoke no connection, because the principal
still authenticates.

The relay SHALL NOT delete any credential file. A credential written at
registration authenticates nothing once the record is gone, and the relay
cannot know where an operator distributed it.

`drop` is a relay-wide operation: the relay SHALL authorize the connection
principal against an `all`-scoped `drop.peer` grant, and a bundle-relative
`home` grant SHALL be insufficient. The `drop.peer` grant SHALL be distinct
from `new.peer` and `change.psk`; neither SHALL confer the ability to drop.

#### Scenario: Advertise drop tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `drop`

#### Scenario: Drop a registered principal

- **WHEN** a caller invokes `drop` with `command="peer"` and a registered
  `principal_id`
- **THEN** the relay deletes the principal's store record
- **AND** the principal's credential no longer authenticates

#### Scenario: Dropping a principal revokes its live sessions

- **WHEN** a principal with an active bound session is dropped
- **THEN** the relay emits a `runtime_identity_revoked` typed error frame to
  that session before closing it
- **AND** emits `identity.revoked` to trusted-host streams whose scope covers
  the principal

#### Scenario: Reject dropping an unregistered principal

- **WHEN** a caller invokes `drop` for a `principal_id` that is not registered
- **THEN** the relay returns `validation_unknown_principal`

#### Scenario: Reject self-drop

- **WHEN** a caller invokes `drop` for the principal it authenticated as
- **THEN** the relay returns `validation_self_drop_forbidden`
- **AND** does not delete the principal
- **AND** does not revoke any connection

#### Scenario: Self-drop is rejected ahead of an authorization denial

- **WHEN** a caller holding no `drop.peer` grant invokes `drop` for the
  principal it authenticated as
- **THEN** the relay returns `validation_self_drop_forbidden` rather than
  `authorization_forbidden`

#### Scenario: Unknown principal is not disclosed to an unauthorized caller

- **WHEN** a caller holding no `drop.peer` grant invokes `drop` for a
  `principal_id` that is not registered
- **THEN** the relay returns `authorization_forbidden` rather than
  `validation_unknown_principal`

#### Scenario: Reject dropping without the drop grant

- **WHEN** a caller holding `all`-scoped `new.peer` and `change.psk` grants but
  no `drop.peer` grant invokes `drop`
- **THEN** the relay returns `authorization_forbidden`
- **AND** does not delete the principal

#### Scenario: Dropping a principal leaves credential files in place

- **WHEN** a principal whose PSK was written to a file is dropped
- **THEN** the relay does not delete that file

### Requirement: MCP Drop Success Payload Contract

Successful `drop` responses SHALL include:

- `schema_version`
- `principal_id`
- `principal_type`
- `credential_path` (present only for **session** principals, reporting the
  relay-owned canonical credential file the operator may want to delete)

`credential_path` SHALL be omitted for relay, user, and application principals.
The relay owns a canonical credential location only for session principals; a
peer relay's credential lives under the *connecting* relay's state root, which
the dropping relay cannot observe. Reporting a path derived from the dropping
relay's own state root would name a file that is not the operator's credential,
so the relay SHALL report no path rather than a locally-derived one.

#### Scenario: Return drop payload for drop peer

- **WHEN** a `drop` `command="peer"` request succeeds
- **THEN** the response includes the required drop fields

#### Scenario: Report the canonical credential path for a dropped session

- **WHEN** a dropped principal is a session principal
- **THEN** the response reports its relay-owned canonical credential path

#### Scenario: Omit the credential path for a dropped peer relay

- **WHEN** a dropped principal is a relay, user, or application principal
- **THEN** the response omits `credential_path`
- **AND** reports no path derived from the dropping relay's state root
