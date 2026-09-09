## MODIFIED Requirements

### Requirement: MCP New Tool

The system SHALL expose a meta-tool `new` that registers a principal credential.
`new` SHALL require `command="peer"`.

`new peer` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)
- `scope` (optional string)
- `output_path` (optional, absolute path)
- `write_to_config` (optional, boolean)

`output_path` and `write_to_config` SHALL be mutually exclusive; a request
supplying both SHALL be rejected with `validation_invalid_params` before any
relay request is issued.

For a relay principal, `scope` SHALL use the `relay-identity` capability's
`Peer Ingress Scope Grammar`: `*` or comma-separated concrete namespaces,
with empty/absent scope granting no rights. The CLI SHALL use the same grammar
through `new peer <principal_id> --scope SCOPE`. Application scope semantics
SHALL remain unchanged. CLI, MCP and the relay SHALL validate peer scope with
the same grammar; adapters SHALL reject malformed input before relay submission
and the relay SHALL validate independently before mutation. Errors SHALL use
`validation_invalid_params` with the scope field identified.

Peer scope is not a policy tier. For a single namespace equal to `none`, `self`,
`home`, or `all`, the response SHALL carry the diagnostic
`advisory_scope_resembles_policy_tier` and the relay SHALL register it anyway.
Those strings are legal namespace names, so the diagnostic SHALL NOT fail the
request; `all` names a namespace, while `*` grants wildcard reach. A scope
merely resolving to nothing SHALL NOT produce the advisory.

Diagnostics SHALL travel in the response payload, not relay process output.
Each SHALL carry a code and human-readable message. MCP SHALL preserve them in
its structured result; CLI SHALL render each to its own stderr and still exit
zero on success.

The relay SHALL generate the PSK, persist only its SHA-256 hash, and route the
raw value to one of three destinations:

- Response (default): return the raw PSK once in the response.
- Path (`output_path`): write the PSK there and omit it from the response. The
  path MUST be absolute, its parent MUST exist, and its target MUST NOT be a
  symlink. Failure of these preconditions SHALL return
  `validation_invalid_output_path`.
- Config (`write_to_config`): write to the relay-owned canonical credential
  path and omit the PSK from the response. This is available only to session
  principals. Relay, user or application principals SHALL receive
  `validation_config_destination_unsupported`. Before deriving any path the
  relay SHALL reject principal components that violate the session-id or
  bundle-name grammar (including traversal-only `.`/`..` and path separators,
  while permitting dotted bundle names) with `validation_invalid_principal_id`.

The relay SHALL validate and stage the destination before store mutation; a
rejected or failed destination SHALL NOT register the principal. Registration
SHALL require the connection principal's relay-wide `new.peer=all` control;
`home` is insufficient. Scope-update authority SHALL NOT substitute for it.

#### Scenario: Advertise new tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the inventory includes `new`

#### Scenario: Mint PSK for a new peer principal

- **WHEN** a caller invokes `new` with `command="peer"` and a principal id
- **THEN** the relay registers the principal and returns the minted PSK
- **AND** omits the PSK from the response for output_path or write_to_config

#### Scenario: Warn on a scope drawn from the policy-tier vocabulary

- **WHEN** a caller registers scope `none`, `self`, `home`, or `all`
- **THEN** the response carries `advisory_scope_resembles_policy_tier`
- **AND** registers the principal with that namespace scope successfully

#### Scenario: MCP caller receives the scope diagnostic

- **WHEN** MCP registration produces the scope advisory
- **THEN** the structured result carries its code and message

#### Scenario: CLI renders the scope diagnostic to stderr

- **WHEN** CLI registration produces the scope advisory
- **THEN** CLI renders it to stderr and exits zero

#### Scenario: Register an unresolvable scope without a diagnostic

- **WHEN** a caller registers a scope naming a namespace not currently present
- **THEN** registration succeeds without the vocabulary-collision diagnostic

#### Scenario: Reject config destination for a non-session principal

- **WHEN** a caller requests write_to_config for a non-session principal
- **THEN** the relay returns `validation_config_destination_unsupported`
- **AND** does not register it

#### Scenario: Reject mutually exclusive credential destinations

- **WHEN** a caller supplies output_path and write_to_config together
- **THEN** the adapter returns `validation_invalid_params`
- **AND** no relay request is issued

#### Scenario: Provision wildcard and namespace sets through CLI and MCP

- **WHEN** CLI uses `new peer west@RELAY --scope 'alpha,beta'` or MCP supplies
  equivalent string scope
- **THEN** both register the same canonical namespace set
- **AND** `--scope '*'` or MCP `scope="*"` grants all addressable namespaces,
  including GLOBAL and future addressable namespace types

### Requirement: MCP Change Tool

The system SHALL expose a meta-tool `change` with `command="psk"` for credential
rotation and `command="scope"` for in-place peer scope replacement. The latter
SHALL follow `MCP Scope Update Contract`. This requirement's remaining
credential lifecycle obligations apply to `command="psk"` only.

`change psk` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)
- `output_path` (optional, absolute path)
- `write_to_config` (optional, boolean)

`output_path` and `write_to_config` SHALL be mutually exclusive; a request
supplying both SHALL be rejected with `validation_invalid_params` before any
relay request is issued.

The relay SHALL generate a new PSK for the existing principal and apply the same
credential-destination selector as `new peer` (Response by default, Path for
output_path, Config for write_to_config), with identical session-only Config
derivation, path preconditions, and safe-segment validation. It SHALL stage and
commit the destination before revoking live connections holding the prior
credential, other than the requester. A rejected or failed destination SHALL
NOT rotate the PSK or revoke any connection. Rotation is relay-wide and SHALL
require `change.psk=all`; home SHALL be insufficient.

Rotation SHALL preserve the latest committed scope, expiry, type and unrelated
metadata, serialized against concurrent scope updates. Rotation rights SHALL
NOT grant scope administration. Rotation replaces a credential while the
principal persists, so it is not revocation under `relay-identity`'s
`Revocation and Expiry Enforcement` requirement; its teardown obligations follow.

The relay SHALL tear down every session authenticated with the prior credential
other than the requester. Before closing each it SHALL emit a
`runtime_identity_revoked` typed error frame. A bare connection drop without the
typed frame is not permitted, being indistinguishable from relay_unavailable.

The relay SHALL emit `identity.revoked` on the stream-event carrier to each
connected trusted-host stream whose scope covers the rotated principal. Each
event SHALL carry the principal_id and revocation timestamp.

The relay SHALL NOT tear down the requesting connection on self-rotation: it
awaits the response carrying the only new PSK copy with Response destination.
Tearing it down discards that response after the hash was committed. The relay
SHALL still emit identity.revoked to trusted hosts so their credential caches
are invalidated. Excluding the requester cannot leave another live session
holding the old credential because the registry admits at most one connection
per principal_id.

#### Scenario: Advertise change tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the inventory includes `change`

#### Scenario: Rotate PSK for an existing principal

- **WHEN** a caller invokes change with command psk and a principal_id
- **THEN** the relay rotates its PSK and returns the new value
- **AND** omits it from the response when a file destination was selected

#### Scenario: Rejected destination leaves the credential unrotated

- **WHEN** a change psk request selects a rejected destination
- **THEN** the relay returns the corresponding validation error
- **AND** does not rotate the PSK or revoke any connection

#### Scenario: Self-rotation returns the rotated credential

- **WHEN** a principal authenticated with its own credential rotates its own
  PSK with Response destination
- **THEN** the relay returns the new PSK on that connection
- **AND** does not tear it down

#### Scenario: Rotation revokes another principal's live session

- **WHEN** a caller rotates a different principal with a live session
- **THEN** that session receives runtime_identity_revoked before closing

#### Scenario: Rotation fans out identity.revoked to trusted hosts

- **WHEN** a caller rotates a principal's PSK
- **THEN** each connected trusted host whose scope covers it receives
  identity.revoked with principal_id and revocation timestamp

#### Scenario: Self-rotation still notifies trusted hosts

- **WHEN** a principal rotates its own PSK
- **THEN** trusted-host streams within scope still receive identity.revoked
- **AND** the requester is not torn down

#### Scenario: Rotation never restores an obsolete grant

- **WHEN** a scope update and PSK rotation overlap
- **THEN** serialization preserves both successful mutations
- **AND** rotation cannot widen scope or overwrite it with a stale copy

### Requirement: MCP Change Success Payload Contract

Successful `change` responses with `command="psk"` SHALL include:

- `schema_version`
- `principal_id`
- `psk` (present only for Response destination)
- `written_path` (present only when the PSK was written to a file)

Success with `command="scope"` SHALL use `MCP Scope Update Contract`, not a
credential-bearing payload.

#### Scenario: Return rotated credential payload for change psk

- **WHEN** change with command psk succeeds
- **THEN** the response includes the required rotated credential fields

## ADDED Requirements

### Requirement: MCP Scope Update Contract

The MCP `change` tool SHALL accept `command="scope"` with exactly required
`args.principal_id` (a peer `<id>@RELAY`) and `args.scope` (string). The CLI
equivalent SHALL be `agentmux change scope <principal_id> --scope SCOPE`,
with existing runtime-root, requester-selection and JSON-output conventions.
Relay `ChangeScope` SHALL carry those two fields and dispatch relay-wide,
without requiring a routing bundle.

Scope SHALL follow `Peer Ingress Scope Grammar`: an explicit empty string clears
rights, while omitted/null input is an invalid update. CLI/MCP SHALL validate
shape and grammar before relay submission; the relay SHALL independently
validate and enforce `Peer Scope Administration Authorization`. Invalid input
SHALL produce `validation_invalid_params` identifying the field. A missing
target record SHALL produce `validation_unknown_principal` after authorization.
Only a registered relay principal can be updated by this command.

The relay SHALL execute `Authoritative Peer Scope Updates` without generating,
returning or writing a PSK, modifying expiry/metadata, disconnecting the peer,
or changing other records. Success SHALL return `schema_version`, `principal_id`
and canonical `scope` (empty string for no rights). No-op replacement SHALL
succeed. Vocabulary diagnostics SHALL follow the New tool's existing
code/message and CLI stderr/MCP structured-result convention when applicable.
Relay authorization and pre-rename persistence failures SHALL propagate through
existing CLI/MCP error handling without claiming success or mutating the old
grant. Post-rename directory-sync failure is excepted: it SHALL propagate as
indeterminate-durability uncertainty carrying the effective replacement scope
while the replacement remains effective.

#### Scenario: Update through CLI or MCP without reissuing a credential

- **WHEN** an authorized caller uses `change scope west@RELAY --scope '*'` or
  MCP command scope with equivalent args
- **THEN** the relay commits the replacement and returns canonical `scope="*"`
- **AND** the PSK, identity, expiry, metadata and connection remain unchanged

#### Scenario: Clear requires an explicit input

- **WHEN** an authorized caller supplies empty scope to change scope
- **THEN** the response reports empty scope and subsequent ingress is denied
- **AND** omitting scope or supplying null is a validation error, not a clear

#### Scenario: Update cannot use a rotation-only permission

- **WHEN** a caller has change.psk=all but lacks change.scope=all
- **THEN** the update returns authorization_forbidden with capability change.scope
- **AND** the peer's record is untouched

#### Scenario: Unknown peer is reported only after authorization

- **WHEN** an authorized caller updates an unregistered peer
- **THEN** the relay returns validation_unknown_principal
- **AND** an unauthorized caller receives authorization denial instead

#### Scenario: Failed update before publication does not publish a grant

- **WHEN** the relay cannot persist a requested scope replacement before rename
- **THEN** CLI/MCP surfaces the failure and not a success payload
- **AND** the old record and effective grant remain unchanged

#### Scenario: Directory-sync failure reports indeterminate durability

- **WHEN** rename publication succeeds but parent-directory sync fails
- **THEN** CLI/MCP surfaces an indeterminate-durability error with the
  effective replacement scope, not success
- **AND** the replacement remains the effective grant

### Requirement: Scope Administration Help Discovery

MCP `help` SHALL advertise scope alongside psk in the `change` command catalog,
and `help change.scope` SHALL expose its exact argument schema and invoke shape.
The schema SHALL require principal_id and scope and disallow unknown fields.
`help new.peer` SHALL document the peer string grammar. CLI new/change help
SHALL match actual dispatch and describe wildcard, sets and explicit clearing.
Help SHALL explain that `*` includes GLOBAL and all future addressable namespace
types, and that change.scope permission is distinct from rotation permission.

#### Scenario: Discover the scope update without prior command knowledge

- **WHEN** a caller asks for change help
- **THEN** CLI and MCP expose both psk and scope commands
- **AND** MCP help change.scope returns its exact required-argument schema

#### Scenario: Discover wildcard consequences

- **WHEN** a caller reads new-peer or scope-update help
- **THEN** it documents sets, wildcard coverage of GLOBAL and future addressable
  types, explicit clearing, and separate update authorization
