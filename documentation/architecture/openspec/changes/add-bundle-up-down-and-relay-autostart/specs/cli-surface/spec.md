## MODIFIED Requirements

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

## ADDED Requirements

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
