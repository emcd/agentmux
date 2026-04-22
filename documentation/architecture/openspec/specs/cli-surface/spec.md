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

For explicit `--target` mode, tokens SHALL be canonical send target
identifiers only.
Configured session `name` values and display-name aliases are not valid
explicit send targets.

Send authorization SHALL follow requester policy control scope:

- `all:home`
- `all:all`

#### Scenario: Send to explicit targets

- **WHEN** a caller invokes `agentmux send` with `--target` values
- **THEN** the system routes to exactly those selected recipients

#### Scenario: Reject configured name alias token for send target

- **WHEN** a caller invokes `agentmux send --target <configured-session-name>`
- **THEN** CLI surfaces `validation_unknown_target`

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

CLI `list sessions` SHALL map to capability `list.read` for relay-handled
single-bundle requests.
If requester identity is valid and policy denies list access, CLI SHALL surface
`authorization_forbidden` and SHALL NOT render a successful session list.

#### Scenario: Return authorization denial for single-bundle list sessions request

- **WHEN** operator invokes `agentmux list sessions`
- **AND** policy denies list visibility for resolved requester identity
- **THEN** CLI returns `authorization_forbidden`
- **AND** does not present successful `bundle.sessions[]` output

### Requirement: CLI ACP Look Success Surface

For look success payloads, CLI machine output SHALL preserve relay payloads
unchanged, including discriminator and variant fields.

When relay returns tmux look payload:
- `snapshot_format="lines"` with `snapshot_lines`.

When relay returns ACP look payload:
- `snapshot_format="acp_entries_v1"` with `snapshot_entries`.

For ACP look responses, CLI machine output SHALL preserve relay additive
freshness fields unchanged:

- `freshness` (`fresh` | `stale`) (required)
- `snapshot_source` (`live_buffer` | `none`) (required)
- `stale_reason_code` (required when `freshness=stale`; absent otherwise)
- `snapshot_age_ms` (optional; omitted when relay omits)

CLI MAY render ACP `snapshot_entries` with local presentation enhancements
(including ANSI/SGR styling), but wire/machine payloads SHALL remain unchanged.

#### Scenario: Preserve ACP structured payload in CLI machine output

- **WHEN** operator runs `agentmux look <target-session>` and ACP payload is
  returned from relay
- **THEN** CLI returns successful look payload unchanged
- **AND** includes `snapshot_format="acp_entries_v1"` and `snapshot_entries`

#### Scenario: Preserve stale-success with empty ACP snapshot entries

- **WHEN** operator runs `agentmux look <target-session>` for ACP target and
  relay returns stale-success with `snapshot_entries=[]`
- **THEN** CLI returns successful look payload
- **AND** includes required ACP freshness fields

#### Scenario: Preserve existing tmux look success path unchanged

- **WHEN** operator runs `agentmux look <target-session>` and target resolves
  to tmux transport
- **THEN** CLI returns canonical successful look payload with
  `snapshot_format="lines"` and `snapshot_lines`

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

`agentmux tui` SHALL support session/bundle selectors:

- optional `--as-session <session-selector>`
- optional `--bundle <bundle-id>`

`agentmux tui --sender` SHALL NOT be supported in MVP.

Bundle selection SHALL resolve as:

1. explicit `--bundle`
2. `default-bundle` from global `tui.toml`
3. fail-fast `validation_unknown_bundle`

Session selection SHALL resolve as:

1. explicit `--as-session`
2. `default-session` from global `tui.toml`
3. fail-fast `validation_unknown_session`

Resolved TUI session SHALL provide canonical wire `id` for relay
operations in that process.

#### Scenario: Launch TUI with explicit session and bundle selectors

- **WHEN** an operator runs `agentmux tui --bundle agentmux --as-session user`
- **THEN** startup resolves session `user` on bundle `agentmux`

#### Scenario: Launch TUI from config defaults

- **WHEN** operator runs `agentmux tui` without `--bundle` and `--as-session`
- **AND** `tui.toml` has `default-bundle` and `default-session`
- **THEN** startup resolves both values from config defaults

#### Scenario: Reject missing default session when selector is omitted

- **WHEN** operator runs `agentmux tui` without `--as-session`
- **AND** `default-session` is absent from `tui.toml`
- **THEN** CLI fails fast with `validation_unknown_session`

#### Scenario: Reject sender flag on TUI command

- **WHEN** an operator runs `agentmux tui --sender relay`
- **THEN** CLI rejects invocation as an unknown argument

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

### Requirement: Send Timeout Override Flags by Transport

`agentmux send` SHALL support transport-scoped timeout override flags:

- `--quiescence-timeout-ms <MS>` for tmux delivery quiescence wait behavior
- `--acp-turn-timeout-ms <MS>` for ACP turn-wait behavior

The command SHALL reject requests that provide both flags in one invocation
with `validation_conflicting_timeout_fields`.

Transport-incompatible timeout flags SHALL fail fast with
`validation_invalid_timeout_field_for_transport`.

#### Scenario: Reject conflicting timeout flags

- **WHEN** an operator invokes `agentmux send` with both
  `--quiescence-timeout-ms` and `--acp-turn-timeout-ms`
- **THEN** invocation fails with `validation_conflicting_timeout_fields`

#### Scenario: Reject tmux timeout flag for ACP target

- **WHEN** `agentmux send` targets ACP-backed session
- **AND** operator provides `--quiescence-timeout-ms`
- **THEN** invocation fails with
  `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject ACP timeout flag for tmux target

- **WHEN** `agentmux send` targets tmux-backed session
- **AND** operator provides `--acp-turn-timeout-ms`
- **THEN** invocation fails with
  `validation_invalid_timeout_field_for_transport`

### Requirement: CLI ACP Sync Delivery-Phase Passthrough

For sync ACP sends, CLI SHALL preserve relay-authored early-success markers in
structured output.

When relay returns phase-1 acknowledgment, CLI JSON output SHALL include:

- `outcome = delivered`
- `details.delivery_phase = "accepted_in_progress"`
- unchanged `message_id` for request tracing

#### Scenario: Preserve relay delivery-phase details in CLI JSON output

- **WHEN** operator invokes `agentmux send --delivery-mode sync` to ACP target
- **AND** relay returns phase-1 acknowledgment marker
- **THEN** CLI JSON output includes
  `details.delivery_phase = "accepted_in_progress"`
- **AND** preserves the same `message_id`

### Requirement: Send Session Selector Surface

`agentmux send` SHALL support optional sender session selector:

- `--as-session <session-selector>`

`agentmux send --sender` SHALL NOT be supported in MVP.

Send bundle resolution SHALL be:

1. explicit `--bundle`
2. `default-bundle` from global `tui.toml`
3. fail-fast `validation_unknown_bundle`

Send session resolution SHALL be:

1. explicit `--as-session`
2. `default-session` from global `tui.toml`
3. fail-fast `validation_unknown_session`

Resolved session `id` SHALL be used as send caller identity before
relay dispatch.

#### Scenario: Send with explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --as-session user --target mcp --message "hi"`
- **AND** session `user` is configured in global TUI sessions
- **THEN** send caller identity resolves as session `user`

#### Scenario: Send with default session fallback

- **WHEN** an operator runs `agentmux send --target mcp --message "hi"`
- **AND** `default-bundle` is defined in `tui.toml`
- **AND** `default-session` is defined in `tui.toml`
- **THEN** send caller identity resolves from that default session

#### Scenario: Reject missing default bundle for send

- **WHEN** an operator runs `agentmux send --as-session user --target mcp --message "hi"`
- **AND** `default-bundle` is absent from `tui.toml`
- **THEN** CLI rejects invocation with `validation_unknown_bundle`

#### Scenario: Reject unknown explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --as-session missing --target mcp --message "hi"`
- **AND** `tui.toml` has no matching `[[sessions]]` selector
- **THEN** CLI rejects invocation with `validation_unknown_session`

#### Scenario: Reject sender flag on send command

- **WHEN** an operator runs `agentmux send --sender relay --target mcp --message "hi"`
- **THEN** CLI rejects invocation as an unknown argument

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
  - `startup_health` (`healthy`|`degraded`) (required when `state=up`;
    omitted when `state=down`)
  - `state_reason_code` (required when `state=down`; omitted when `state=up`)
  - `state_reason` (optional)
  - `startup_failure_count` (required integer)
  - `recent_startup_failures` (required array; may be empty)
  - `sessions[]` with `id`, `name?`, `transport`

Each `recent_startup_failures[]` entry SHALL include:

- `bundle_name`
- `session_id`
- `transport` (`tmux`|`acp`)
- `code`
- `reason`
- `timestamp`
- `sequence`
- optional `details`

For `--all` mode, CLI machine output SHALL include:

- `schema_version`
- `bundles[]` (array of canonical single-bundle `bundle` objects)

`bundles[]` ordering SHALL be lexicographic by bundle id.

#### Scenario: Return startup health and startup-failure fields in single-bundle output

- **WHEN** operator invokes `agentmux list sessions --bundle <bundle-id>`
- **THEN** CLI output includes required startup health/state fields
- **AND** includes required startup failure history fields

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

Home-bundle fallback startup-failure fields
(`startup_failure_count`, `recent_startup_failures`) SHALL be treated as
best-effort synthesized values from available local runtime state. When local
runtime failure history is unavailable, CLI SHALL return:

- `startup_failure_count=0`
- `recent_startup_failures=[]`

#### Scenario: Synthesize canonical home-bundle payload when relay is unreachable

- **WHEN** operator requests associated/home bundle session listing
- **AND** bundle relay is unreachable
- **THEN** CLI returns canonical single-bundle payload with `state=down`
- **AND** includes required startup failure fields

#### Scenario: Default fallback startup-failure fields when local history is unavailable

- **WHEN** home-bundle fallback is synthesized
- **AND** local runtime startup-failure history cannot be read
- **THEN** CLI returns `startup_failure_count=0`
- **AND** returns `recent_startup_failures=[]`

#### Scenario: Reject non-home unreachable fallback synthesis

- **WHEN** target bundle is not associated/home bundle
- **AND** bundle relay is unreachable
- **THEN** CLI returns `relay_unavailable`

