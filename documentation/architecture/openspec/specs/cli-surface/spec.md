# cli-surface Specification

## Purpose
TBD - created by archiving change add-agentmux-cli-host-send-mvp. Update Purpose after archive.
## Requirements
### Requirement: Unified Agentmux Command Topology

The system SHALL provide a primary `agentmux` CLI command with these
subcommands:

- `host relay`
- `host mcp`
- `up`
- `down`
- `list`
- `send`
- `look`

The system SHALL retain `agentmux-relay` and `agentmux-mcp` as compatibility
entrypoints.

#### Scenario: Expose bundle lifecycle commands in topology

- **WHEN** an operator views `agentmux --help`
- **THEN** the CLI includes `up` and `down` subcommands

#### Scenario: Host relay from unified command

- **WHEN** an operator runs `agentmux host relay`
- **THEN** the system starts relay hosting flow

#### Scenario: Host MCP from unified command

- **WHEN** an operator runs `agentmux host mcp`
- **THEN** the system starts MCP hosting flow with configured association
  resolution

#### Scenario: Preserve legacy binary entrypoints

- **WHEN** an operator runs `agentmux-relay` or `agentmux-mcp`
- **THEN** the command remains supported
- **AND** behavior remains equivalent to the unified host command paths

### Requirement: Relay Host Bundle Selection

`agentmux host relay` SHALL be no-selector command in MVP.

`agentmux host relay` SHALL accept optional `--no-autostart`.

In no-selector mode:

- default behavior autostarts eligible bundles
- `--no-autostart` disables bundle autostart while still starting relay process

#### Scenario: Start relay with default no-selector autostart mode

- **WHEN** an operator runs `agentmux host relay`
- **THEN** the system starts relay process
- **AND** evaluates autostart-eligible bundles for hosting

#### Scenario: Start relay process without bundle autostart

- **WHEN** an operator runs `agentmux host relay --no-autostart`
- **THEN** the system starts relay process
- **AND** does not host bundles as part of startup

#### Scenario: Reject bundle selector argument for host relay

- **WHEN** an operator runs `agentmux host relay relay`
- **THEN** the system rejects invocation with structured argument validation
  error

#### Scenario: Reject group selector flag for host relay

- **WHEN** an operator runs `agentmux host relay --group dev`
- **THEN** the system rejects invocation with structured argument validation
  error

### Requirement: Send Target Mode Selection

`agentmux send` SHALL support exactly one target mode per request:

- one or more explicit `--target` values
- `--broadcast`

Send authorization SHALL follow requester policy control scope:

- `all:home`
- `all:all`

#### Scenario: Send to explicit targets

- **WHEN** a caller invokes `agentmux send` with `--target` values
- **THEN** the system routes to exactly those selected recipients

#### Scenario: Send as broadcast

- **WHEN** a caller invokes `agentmux send --broadcast`
- **THEN** the system routes to bundle recipients excluding sender

#### Scenario: Reject conflicting target modes

- **WHEN** a caller provides both explicit `--target` values and `--broadcast`
- **THEN** the system rejects invocation with
  `validation_conflicting_targets`

#### Scenario: Deny cross-bundle send under home-only scope

- **WHEN** caller requests cross-bundle send
- **AND** requester policy `send` scope is `all:home`
- **THEN** CLI surfaces `authorization_forbidden`

### Requirement: Send Message Input Resolution

`agentmux send` SHALL resolve message body from exactly one source:

- `--message`
- piped stdin

If `--message` is omitted and piped stdin is available, stdin content SHALL be
used as the message body.

If both sources are present, the system SHALL reject invocation with
`validation_conflicting_message_input`.

If neither source is present, the system SHALL reject invocation with
`validation_missing_message_input`.

The MVP system SHALL NOT enter interactive line-capture mode when stdin is a
TTY and `--message` is omitted.

#### Scenario: Read message body from option flag

- **WHEN** a caller invokes `agentmux send --message "Hello"`
- **THEN** the system uses the provided flag value as message body

#### Scenario: Read message body from piped stdin

- **WHEN** a caller invokes `agentmux send` without `--message`
- **AND** stdin is piped with non-empty content
- **THEN** the system uses stdin content as message body

#### Scenario: Reject conflicting message sources

- **WHEN** a caller provides `--message`
- **AND** stdin is piped with message content
- **THEN** the system rejects invocation with
  `validation_conflicting_message_input`

#### Scenario: Reject missing message source in non-piped mode

- **WHEN** a caller invokes `agentmux send` without `--message`
- **AND** stdin is a TTY
- **THEN** the system rejects invocation with
  `validation_missing_message_input`

### Requirement: Relay Host Startup Summary Contract

`agentmux host relay` SHALL expose a canonical machine startup summary payload.

The summary SHALL include:

- `schema_version`
- `host_mode` (`autostart`|`process_only`)
- `bundles` array with per-bundle entries:
  - `bundle_name`
  - `outcome` (`hosted`, `skipped`, `failed`)
  - `reason_code` (nullable)
  - `reason` (nullable human text)
- `hosted_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `hosted_any` (boolean)

When a bundle is skipped due to runtime lock contention, `reason_code` SHALL be
`lock_held`.

CLI text output SHALL be a rendering layer over the same summary payload.

In `host_mode=autostart`, process exit status SHALL reflect relay process
startup result and SHALL NOT fail solely because `hosted_bundle_count == 0`.

#### Scenario: Emit startup summary in autostart mode

- **WHEN** relay host starts with no selector
- **THEN** summary payload sets `host_mode=autostart`

#### Scenario: Emit startup summary in process-only mode

- **WHEN** relay host starts with `--no-autostart`
- **THEN** startup outcomes are represented in the canonical machine payload
- **AND** `host_mode` is `process_only`

### Requirement: Relay Host CLI Scope (MVP)

MVP `agentmux host relay` SHALL support:

- no selector (default autostart mode)
- `--no-autostart` (process-only mode)

MVP `agentmux host relay` SHALL NOT support:

- positional `<bundle-id>`
- `--group <GROUP>`
- `--all`
- `--include-bundle`
- `--exclude-bundle`

#### Scenario: Reject all flag for host relay

- **WHEN** an operator passes `--all` to `agentmux host relay`
- **THEN** the system rejects invocation with a structured argument validation
  error

#### Scenario: Reject include-bundle override for host relay

- **WHEN** an operator passes `--include-bundle` to `agentmux host relay`
- **THEN** the system rejects invocation with a structured argument validation
  error

#### Scenario: Reject exclude-bundle override for host relay

- **WHEN** an operator passes `--exclude-bundle` to `agentmux host relay`
- **THEN** the system rejects invocation with a structured argument validation
  error

### Requirement: Look Command Surface

The system SHALL expose a read-only inspection command:

- `agentmux look <target-session>`

`agentmux look` SHALL support:

- optional `--bundle <name>`
- optional `--lines <n>`

`agentmux look` SHALL return canonical structured JSON output in MVP.
`agentmux look` authorization SHALL use capability label `look.inspect`.
Policy control `look` determines allowed scope (`self`, `all:home`, `all:all`).
Cross-bundle look remains currently unsupported by runtime contract.

#### Scenario: Inspect target session from CLI

- **WHEN** an operator runs `agentmux look <target-session>`
- **THEN** the system requests a read-only snapshot for that target session
- **AND** returns structured JSON payload from relay inspection response

#### Scenario: Use associated bundle when bundle flag is omitted

- **WHEN** an operator runs `agentmux look <target-session>` without `--bundle`
- **THEN** the system uses associated bundle context resolved for the caller

#### Scenario: Reject invalid lines value

- **WHEN** an operator provides `--lines` outside valid range
- **THEN** the system rejects invocation with `validation_invalid_lines`

#### Scenario: Reject cross-bundle look attempt in MVP

- **WHEN** an operator provides `--bundle` outside associated bundle context
- **THEN** the system rejects invocation with
  `validation_cross_bundle_unsupported`

#### Scenario: Deny same-bundle non-self look under self scope

- **WHEN** operator requests look for same-bundle non-self target
- **AND** requester policy `look` scope is `self`
- **THEN** CLI surfaces `authorization_forbidden`

### Requirement: CLI Authorization Adapter Boundary

CLI SHALL remain a validator/adapter surface and SHALL perform no independent
authorization decisioning.
Relay SHALL remain the centralized policy decision point.

#### Scenario: Propagate relay authorization denial unchanged

- **WHEN** relay returns `authorization_forbidden`
- **THEN** CLI surfaces the same code and details schema
- **AND** CLI does not implement command-specific authorization branches

### Requirement: List Command Authorization Semantics

CLI `list` SHALL map to capability `list.read` for authorization outcomes.
If requester identity is valid and policy denies list access, CLI SHALL surface
`authorization_forbidden` and SHALL NOT render an empty successful list.

#### Scenario: Return authorization denial for list request

- **WHEN** operator invokes `agentmux list`
- **AND** policy denies list visibility for resolved requester identity
- **THEN** CLI returns `authorization_forbidden`
- **AND** does not present an empty recipient list as success

### Requirement: Bare Agentmux TUI Dispatch

When invoked without a subcommand, `agentmux` SHALL dispatch based on terminal
context:

- interactive TTY invocation starts TUI workflow,
- non-TTY invocation fails fast by printing help and exiting non-zero.

#### Scenario: Launch bare agentmux on TTY

- **WHEN** an operator runs `agentmux` without a subcommand
- **AND** the process is attached to an interactive TTY
- **THEN** the system starts TUI workflow as if `agentmux tui` was invoked

#### Scenario: Launch bare agentmux without TTY

- **WHEN** an operator runs `agentmux` without a subcommand
- **AND** the process is not attached to an interactive TTY
- **THEN** the system prints CLI help output
- **AND** exits with a non-zero status code

### Requirement: TUI Sender Override Precedence Hook

`agentmux tui` SHALL support optional `--sender <session-id>`.

When `--sender` is provided, it SHALL have higher precedence than
configuration-based or association-based sender resolution.

#### Scenario: Launch TUI with explicit sender override

- **WHEN** an operator runs `agentmux tui --sender relay`
- **AND** a sender is also configured via override or normal `tui.toml`
- **THEN** TUI startup sender identity resolves to `relay`

#### Scenario: Launch TUI without explicit sender override

- **WHEN** an operator runs `agentmux tui` without `--sender`
- **THEN** TUI startup sender identity resolves via configured precedence

### Requirement: Bundle Lifecycle Command Surface

The CLI SHALL expose explicit bundle lifecycle commands:

- `agentmux up <bundle-id>`
- `agentmux up --group <GROUP>`
- `agentmux down <bundle-id>`
- `agentmux down --group <GROUP>`

For both `up` and `down`, `<bundle-id>` and `--group` SHALL be mutually
exclusive and exactly one selector mode SHALL be required.

`up/down` SHALL operate against a running relay process.

If relay is unavailable, CLI SHALL return `relay_unavailable`.

#### Scenario: Host one bundle through up command

- **WHEN** operator runs `agentmux up relay`
- **THEN** CLI requests bundle host transition for `relay` on active relay

#### Scenario: Unhost one bundle group through down command

- **WHEN** operator runs `agentmux down --group dev`
- **THEN** CLI requests bundle unhost transition for selected group bundles

#### Scenario: Reject missing selector for up command

- **WHEN** operator runs `agentmux up` with no selector
- **THEN** CLI rejects invocation with structured argument validation error

#### Scenario: Surface relay unavailable for down command

- **WHEN** operator runs `agentmux down --group ALL`
- **AND** relay process is unreachable
- **THEN** CLI returns `relay_unavailable`

### Requirement: Bundle Lifecycle Transition Summary Contract

`agentmux up` and `agentmux down` SHALL return canonical machine payloads.

The payload SHALL include:

- `schema_version`
- `action` (`up`|`down`)
- `bundles` array with per-bundle entries:
  - `bundle_name`
  - `outcome` (`hosted`|`unhosted`|`skipped`|`failed`)
  - `reason_code` (nullable)
  - `reason` (nullable)
- `changed_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `changed_any` (boolean)

`up/down` SHALL be idempotent:

- already hosted bundle in `up` returns `outcome=skipped` with
  `reason_code=already_hosted`
- already unhosted bundle in `down` returns `outcome=skipped` with
  `reason_code=already_unhosted`

CLI text output SHALL be a rendering layer over the same payload.

#### Scenario: Report idempotent already-hosted result for up

- **WHEN** operator runs `agentmux up relay`
- **AND** bundle `relay` is already hosted
- **THEN** result includes `outcome=skipped`
- **AND** `reason_code=already_hosted`

#### Scenario: Report idempotent already-unhosted result for down

- **WHEN** operator runs `agentmux down relay`
- **AND** bundle `relay` is already unhosted
- **THEN** result includes `outcome=skipped`
- **AND** `reason_code=already_unhosted`

