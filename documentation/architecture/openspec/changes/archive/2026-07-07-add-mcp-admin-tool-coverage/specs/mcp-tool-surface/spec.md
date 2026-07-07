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

## ADDED Requirements

### Requirement: MCP Updown Tool

The system SHALL expose a meta-tool `updown` that administers the associated
bundle's runtime state. `updown` SHALL require `command`:

- `command="up"` requests hosting the associated bundle runtime.
- `command="down"` requests unhosting the associated bundle runtime.

`updown` SHALL address only the MCP server's associated bundle; cross-bundle
administration is out of scope. The deciding principal SHALL be the caller
session carried by the MCP server's Hello-established relay connection, and the
relay SHALL authorize it against the `updown` policy control (deny by default).

#### Scenario: Advertise updown tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `updown`

#### Scenario: Reject missing updown command selector

- **WHEN** a caller invokes `updown` without `command="up"` or `command="down"`
- **THEN** the tool returns `validation_invalid_params`

#### Scenario: Preserve updown authorization denial capability label

- **WHEN** the relay denies an `updown` request by policy
- **THEN** the tool returns `authorization_forbidden`
- **AND** denial details preserve `capability = "updown"`

### Requirement: MCP Updown Success Payload Contract

Successful `updown` responses SHALL preserve the relay bundle-transition payload
unchanged:

- `schema_version`
- `action`
- `bundles`
- `changed_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `changed_any`

#### Scenario: Return bundle-transition payload for updown

- **WHEN** an `updown` request succeeds
- **THEN** the response includes the required bundle-transition fields

### Requirement: MCP New Tool

The system SHALL expose a meta-tool `new` that registers a principal credential.
`new` SHALL require `command="peer"`.

`new peer` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)
- `scope` (optional)
- `output_path` (optional, absolute path)

The relay SHALL generate the PSK, persist only its SHA-256 hash, and return the
raw PSK once. When `output_path` is provided, the relay SHALL write the PSK to
that path and omit it from the response. `new` is a relay-wide operation: the
relay SHALL authorize the connection principal against an `all`-scoped
`new.peer` grant, and a bundle-relative `home` grant SHALL be insufficient.

#### Scenario: Advertise new tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `new`

#### Scenario: Mint PSK for a new peer principal

- **WHEN** a caller invokes `new` with `command="peer"` and a `principal_id`
- **THEN** the relay registers the principal and returns the minted PSK
- **AND** omits the raw PSK from the response when `output_path` was provided

### Requirement: MCP New Success Payload Contract

Successful `new` responses SHALL include:

- `schema_version`
- `principal_id`
- `principal_type`
- `config_snippet`
- `psk` (present unless the PSK was written to `output_path`)
- `output_path` (present only when the PSK was written to a path)

#### Scenario: Return minted credential payload for new peer

- **WHEN** a `new` `command="peer"` request succeeds
- **THEN** the response includes the required credential fields

### Requirement: MCP Change Tool

The system SHALL expose a meta-tool `change` that rotates an existing
principal's PSK. `change` SHALL require `command="psk"`.

`change psk` request `args` SHALL be:

- `principal_id` (required, `<id>@<namespace>`)

The relay SHALL generate a new PSK for the existing principal and return it.
`change` is a relay-wide operation: the relay SHALL authorize the connection
principal against an `all`-scoped `change.psk` grant, and a bundle-relative
`home` grant SHALL be insufficient.

#### Scenario: Advertise change tool

- **WHEN** an MCP client enumerates available tools
- **THEN** the system includes `change`

#### Scenario: Rotate PSK for an existing principal

- **WHEN** a caller invokes `change` with `command="psk"` and a `principal_id`
- **THEN** the relay rotates the principal's PSK and returns the new value

### Requirement: MCP Change Success Payload Contract

Successful `change` responses SHALL include:

- `schema_version`
- `principal_id`
- `psk`

#### Scenario: Return rotated credential payload for change psk

- **WHEN** a `change` `command="psk"` request succeeds
- **THEN** the response includes the required rotated credential fields
