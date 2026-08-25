# cli-surface Specification

## Purpose
The operator-facing `agentmux` command surface plus the legacy `agentmux-relay`/`agentmux-mcp` binary entrypoints. The spec governs each subcommand's argument validation, target-mode selection, response payload contracts, and authorization passthrough to relay, one requirement per command surface. The credential-administration commands (`new peer`, `change psk`, `drop peer`) are the exception: their contracts live in `mcp-tool-surface` alongside the meta-tools they mirror, and `cli-surface` covers only their presence in the help topology. The CLI is a thin adapter throughout: it surfaces relay decisions unchanged and resolves `--as-session`/`--bundle` selectors against `tui.toml` defaults.
## Requirements
### Requirement: Unified Agentmux Command Topology

The system SHALL provide a primary `agentmux` CLI command that dispatches the
operator subcommands, and SHALL retain `agentmux-relay` and `agentmux-mcp` as
compatibility entrypoints.

`agentmux --help` SHALL list every subcommand the CLI dispatches, and SHALL list
no subcommand it does not dispatch. The help topology is the discovery surface:
a command reachable only by knowing its name already is unusable to the operator
who needed to discover it.

This requirement does not enumerate the subcommands. Each command's surface —
its arguments, validation, and output contracts — is specified by its own
requirement, so a roster here would restate them, and the restatement is what
drifts: it is the enumeration, not the commands, that fell five behind the
shipped CLI. Leaving it out also means adding a command no longer requires
modifying this requirement, which is what previously put concurrent proposals
into conflict over it.

#### Scenario: Help topology matches the dispatched subcommands

- **WHEN** an operator views `agentmux --help`
- **THEN** the listed subcommands are exactly those the CLI dispatches
- **AND** every listed subcommand is reachable by dispatch

#### Scenario: Host relay from unified command

- **WHEN** an operator runs `agentmux host relay`
- **THEN** the system starts relay hosting flow

#### Scenario: Host MCP from unified command

- **WHEN** an operator runs `agentmux host mcp`
- **THEN** the system starts MCP hosting flow with configured association
  resolution

#### Scenario: Pre-flight configuration from unified command

- **WHEN** an operator runs `agentmux check configuration`
- **THEN** the system validates bundle configuration without starting the relay

#### Scenario: Preserve legacy binary entrypoints

- **WHEN** an operator runs `agentmux-relay` or `agentmux-mcp`
- **THEN** the command remains supported
- **AND** behavior remains equivalent to the unified host command paths

### Requirement: Relay Host Bundle Selection

`agentmux host relay` SHALL be no-selector command.

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

- `home`
- `all`

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
- **AND** requester policy `send` scope is `home`
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

The system SHALL NOT enter interactive line-capture mode when stdin is a
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
  - `outcome` (`hosted`, `degraded`, `skipped`, `failed`)
  - `reason_code` (nullable)
  - `reason` (nullable human text)
  - `details` (nullable structured error details, preserved from the
    underlying relay error when one caused the outcome, or carrying
    `failed_sessions` when startup recorded per-session failures)
- `hosted_bundle_count`
- `degraded_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `hosted_any` (boolean)

`outcome=degraded` SHALL be reported for a bundle in which at least one
configured session reached ready state and at least one session startup attempt
failed. A partially started bundle SHALL NOT be reported as `hosted`. It remains
a hosted outcome: `hosted_any` SHALL be true when
`hosted_bundle_count + degraded_bundle_count > 0`, and a degraded bundle SHALL
NOT be counted in `failed_bundle_count`.

When a bundle's startup recorded per-session failures, `reason` SHALL name each
failed session and its cause, and `details.failed_sessions` SHALL carry the
structured per-session records. This SHALL hold for a `degraded` outcome as well
as a `failed` one, so the per-session causes reach the operator from the startup
summary itself rather than only from a subsequent `list`.

When a bundle is skipped due to runtime lock contention, `reason_code` SHALL be
`lock_held`.

CLI text output SHALL be a rendering layer over the same summary payload, and
SHALL render the failed session ids and causes for a `degraded` or `failed`
bundle.

In `host_mode=autostart`, process exit status SHALL reflect relay process
startup result and SHALL NOT fail solely because `hosted_bundle_count == 0`, nor
solely because a bundle is `degraded`.

When startup fails for one or more bundles and the host exits, each failed
bundle SHALL leave a per-bundle reason on stderr and in inscriptions before
the process exits.

#### Scenario: Emit startup summary in autostart mode

- **WHEN** relay host starts with no selector
- **THEN** summary payload sets `host_mode=autostart`

#### Scenario: Emit startup summary in process-only mode

- **WHEN** relay host starts with `--no-autostart`
- **THEN** startup outcomes are represented in the canonical machine payload
- **AND** `host_mode` is `process_only`

#### Scenario: Emit per-bundle failure reasons before fatal startup exit

- **WHEN** every autostart bundle fails to start
- **THEN** the host exits nonzero
- **AND** stderr carries a per-bundle failure reason with the structured
  error details
- **AND** inscriptions record a per-bundle startup failure event with the
  same structured details

#### Scenario: Report a partially started bundle as degraded

- **WHEN** an autostart bundle has at least one session reach ready state and
  at least one session startup attempt fail
- **THEN** the summary entry uses `outcome=degraded`
- **AND** `reason` names each failed session and its cause
- **AND** `details.failed_sessions` carries the structured per-session records
- **AND** the bundle is counted in `degraded_bundle_count`
- **AND** `hosted_any` is true
- **AND** the host does not exit nonzero on account of the degraded bundle

#### Scenario: Render failed session causes in startup text output

- **WHEN** the startup summary contains a `degraded` or `failed` bundle with
  recorded per-session failures
- **THEN** CLI text output names each failed session and its cause

### Requirement: Relay Host CLI Scope

`agentmux host relay` SHALL support:

- no selector (default autostart mode)
- `--no-autostart` (process-only mode)

`agentmux host relay` SHALL NOT support:

- positional `<bundle-id>`
- `--group <GROUP>`

#### Scenario: Run host relay with default autostart mode

- **WHEN** operator runs `agentmux host relay` with no selector
- **THEN** startup autostarts every configured bundle with `autostart: true`

#### Scenario: Run host relay in process-only mode

- **WHEN** operator runs `agentmux host relay --no-autostart`
- **THEN** startup proceeds without autostarting any bundle

#### Scenario: Reject bundle selector argument for host relay

- **WHEN** operator runs `agentmux host relay <bundle-id>`
- **THEN** startup rejects the invocation with `validation_invalid_arguments`

#### Scenario: Reject group selector flag for host relay

- **WHEN** operator runs `agentmux host relay --group <GROUP>`
- **THEN** startup rejects the invocation as an unknown argument

### Requirement: Look Command Surface

The system SHALL expose a read-only inspection command:

- `agentmux look <target-session>`

`agentmux look` SHALL support:

- optional `--bundle <name>` (selects the requester's dispatch bundle, not the
  target's)
- optional `--lines <n>`

`<target-session>` MAY be a bare session id (inspected within the requester's
dispatch bundle) or a peer-qualified `<session>@<bundle>` id that inspects a
session in a peer bundle.

`agentmux look` SHALL return canonical structured JSON output.
`agentmux look` authorization SHALL use capability label `look.inspect`.
Policy control `look` determines allowed scope (`self`, `home`, `all`).
Cross-bundle look (a `<session>@<bundle>` target naming a peer bundle) requires
`all` scope; same-bundle non-self look requires `home`. The CLI is a
thin adapter and propagates relay authorization and resolution outcomes
unchanged.

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

#### Scenario: Inspect peer bundle session via qualified target

- **WHEN** an operator runs `agentmux look <session>@<peer-bundle>` naming a
  bundle other than the requester's dispatch bundle
- **AND** the requester is authorized at `look = all`
- **THEN** the system returns the peer bundle's snapshot from the relay response

#### Scenario: Surface peer resolution errors from relay

- **WHEN** the qualified target names a bundle not configured on the relay, or a
  session that is not a member of the named peer bundle
- **THEN** the CLI surfaces `validation_unknown_bundle` or
  `validation_unknown_target` respectively, unchanged from the relay

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
- `snapshot_format="structured_entries_v1"` with `snapshot_entries`.

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
- **AND** includes `snapshot_format="structured_entries_v1"` and `snapshot_entries`

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

Bundle selection for interactive `agentmux tui` SHALL be lenient — the operator
picks a browsing bundle in the picker, so an absent default is not an error:

1. explicit `--bundle`
2. `default-bundle` from `ui.toml`
3. first available configured bundle
4. empty browsing context when no bundle is available

Session selection SHALL resolve as:

1. explicit `--as-session`
2. `default-session` from `users.toml`
3. fail-fast `validation_unknown_session`

Resolved TUI session SHALL provide canonical wire `id` for relay
operations in that process.

#### Scenario: Launch TUI with explicit session and bundle selectors

- **WHEN** an operator runs `agentmux tui --bundle agentmux --as-session user`
- **THEN** startup resolves session `user` on bundle `agentmux`

#### Scenario: Launch TUI from config defaults

- **WHEN** operator runs `agentmux tui` without `--bundle` and `--as-session`
- **AND** `ui.toml` defines `default-bundle` and `users.toml` defines
  `default-session`
- **THEN** startup resolves both values from config defaults

#### Scenario: Reject missing default session when selector is omitted

- **WHEN** operator runs `agentmux tui` without `--as-session`
- **AND** `default-session` is absent from `users.toml`
- **THEN** CLI fails fast with `validation_unknown_session`

#### Scenario: Fall back to an available bundle when tui default is absent

- **WHEN** operator runs `agentmux tui` without `--bundle`
- **AND** `default-bundle` is absent from `ui.toml`
- **THEN** startup resolves the browsing bundle from the first available
  configured bundle, or an empty browsing context when none is available

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
  - `outcome` (`hosted`|`unhosted`|`degraded`|`skipped`|`failed`)
  - `reason_code` (nullable)
  - `reason` (nullable)
  - `details` (nullable structured detail; carries `failed_sessions` when the
    transition recorded per-session startup failures)
- `changed_bundle_count`
- `degraded_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `changed_any` (boolean)

A configured session that fails to start SHALL NOT fail the `up` transition for
the bundle. Such a bundle SHALL report `outcome=degraded` when at least one
configured session is ready afterward, and `outcome=failed` when none is, per
the `bundle-lifecycle` capability's `Relay Bundle Lifecycle Result Contract`.
`changed_any` SHALL be true when
`changed_bundle_count + degraded_bundle_count > 0`.

`up/down` SHALL be idempotent:

- already hosted bundle in `up` returns `outcome=skipped` with
  `reason_code=already_hosted`
- already unhosted bundle in `down` returns `outcome=skipped` with
  `reason_code=already_unhosted`

CLI text output SHALL be a rendering layer over the same payload, and SHALL
render the failed session ids and causes for a `degraded` or `failed` bundle.

Process exit status SHALL reflect the requested transition: the command SHALL
exit non-zero when `failed_bundle_count > 0`, and SHALL NOT exit non-zero
solely because a bundle is `degraded`, nor solely because
`changed_bundle_count == 0`. A failing transition SHALL emit the same summary
payload a succeeding one does, so the per-bundle detail naming what failed
reaches the caller that has to act on the exit status.

#### Scenario: Exit non-zero when a bundle transition fails

- **WHEN** operator runs `agentmux up relay`
- **AND** the transition reports `outcome=failed` for at least one bundle
- **THEN** the summary payload names the failed bundle and its cause
- **AND** the command exits non-zero

#### Scenario: Exit zero for a partially started transition

- **WHEN** operator runs `agentmux up relay`
- **AND** the transition reports `outcome=degraded` with no `failed` bundle
- **THEN** the command exits zero

#### Scenario: Exit zero when a transition changes nothing

- **WHEN** operator runs `agentmux up relay`
- **AND** every selected bundle reports `outcome=skipped`
- **THEN** the command exits zero

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

#### Scenario: Bring a bundle up when one of its sessions fails

- **WHEN** operator runs `agentmux up relay`
- **AND** one configured session fails to start while another becomes ready
- **THEN** the command does not fail the transition
- **AND** the result entry uses `outcome=degraded`
- **AND** `details.failed_sessions` carries the failed session id and cause
- **AND** CLI text output names that session and its cause

### Requirement: Send Timeout Override Flags by Transport

`agentmux send` SHALL NOT expose any transport-scoped timeout override flag.
Delivery patience is relay configuration, not a per-call or per-coder surface:
the `[delivery]` keys in `relay.toml` (see the `runtime-bootstrap` capability's
`Relay Configuration File` requirement) are the only timeout surfaces.

This change deletes every per-coder timeout key that this requirement previously
enumerated — `[coders.<id>.acp].prime-timeout-ms`,
`[coders.<id>.pty].prime-timeout-ms`, `[coders.<id>.tmux].prime-timeout-ms`, and
`[coders.<id>.tmux].readiness-timeout-ms` — because how long a delivery may wait
is a property of the relay's patience rather than of any coder.

Adding a delivery-patience key SHALL NOT be read as licence to add a per-call
override for it. The no-per-call-override property is the invariant this
requirement states; the enumeration of keys is incidental to it and SHALL be kept
current, so that "the only timeout surfaces" remains a true statement rather than
a stale one. The enumeration SHALL be reconciled against the authoritative
descriptor lists in the `addressing-routing` capability's `Bundle Membership
Configuration` requirement and the `runtime-bootstrap` capability's `Relay
Configuration File` requirement, rather than extended only with the key a change
happens to introduce.

Transport-incompatible timeout flags SHALL fail fast with
`validation_invalid_timeout_field_for_transport`. With no transport-scoped
timeout override flags, this validation class is reserved for a future per-call
override, if one is ever reintroduced.

#### Scenario: Reject retired tmux timeout flag

- **WHEN** `agentmux send` is invoked with `--quiescence-timeout-ms` (a flag that
  does not exist)
- **THEN** invocation fails at the CLI parser as an unknown flag

#### Scenario: Reject retired ACP timeout flag

- **WHEN** `agentmux send` is invoked with `--acp-turn-timeout-ms` (a flag that
  does not exist)
- **THEN** invocation fails at the CLI parser as an unknown flag

#### Scenario: No flag bounds how long a delivery waits

- **WHEN** an operator wants to change how long a delivery waits for a target to
  become ready
- **THEN** no CLI flag and no configuration key offers that control, because the
  wait is unbounded by design
- **AND** `agentmux send` exposes no per-call timeout override of any kind

### Requirement: Send Session Selector Surface

`agentmux send` SHALL support optional sender session selector:

- `--as-session <session-selector>`

Send bundle resolution SHALL be:

1. explicit `--bundle`
2. `default-bundle` from `ui.toml`
3. fail-fast `validation_unknown_bundle`

Send session resolution SHALL be:

1. explicit `--as-session`
2. `default-session` from `users.toml`
3. fail-fast `validation_unknown_session`

Resolved session `id` SHALL be used as send caller identity before
relay dispatch.

#### Scenario: Send with explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --as-session user --target mcp --message "hi"`
- **AND** session `user` is configured in global TUI sessions
- **THEN** send caller identity resolves as session `user`

#### Scenario: Send with default session fallback

- **WHEN** an operator runs `agentmux send --target mcp --message "hi"`
- **AND** `default-bundle` is defined in `ui.toml`
- **AND** `default-session` is defined in `users.toml`
- **THEN** send caller identity resolves from that default session

#### Scenario: Reject missing default bundle for send

- **WHEN** an operator runs `agentmux send --as-session user --target mcp --message "hi"`
- **AND** `default-bundle` is absent from `ui.toml`
- **THEN** CLI rejects invocation with `validation_unknown_bundle`

#### Scenario: Reject unknown explicit session selector

- **WHEN** an operator runs `agentmux send --bundle agentmux --as-session missing --target mcp --message "hi"`
- **AND** `users.toml` has no matching `[[sessions]]` selector
- **THEN** CLI rejects invocation with `validation_unknown_session`

### Requirement: List Sessions Command Surface

The CLI SHALL expose a principals-listing surface:

- `agentmux list principals --namespace <namespace>`

`--namespace` SHALL select the listing scope:

- omitted → associated/home bundle (default)
- a bundle name → that specific bundle
- `GLOBAL` → relay-wide principals
- `*` → adapter-owned fan-out across all namespaces

The prior `--bundle` and `--all` flags are removed with no compatibility alias.
The legacy `agentmux list` surface remains removed.

#### Scenario: Resolve home bundle when namespace is omitted

- **WHEN** operator invokes `agentmux list principals` with no selector
- **THEN** CLI targets associated/home bundle

#### Scenario: Fan out across all namespaces with star token

- **WHEN** operator invokes `agentmux list principals --namespace '*'`
- **THEN** CLI performs adapter-owned fan-out across all namespaces

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
  - `principals[]` with `id`, `name?`, `transport`

Each `recent_startup_failures[]` entry SHALL include:

- `session_id`
- `transport` (`tmux`|`acp`)
- `code`
- `reason`
- `timestamp`
- `sequence`
- optional `details`

For `--namespace '*'` mode, CLI machine output SHALL include:

- `schema_version`
- `bundles[]` (array of canonical single-bundle `bundle` objects)

`bundles[]` ordering SHALL be lexicographic by bundle id.

#### Scenario: Return startup health and startup-failure fields in single-bundle output

- **WHEN** operator invokes `agentmux list principals --namespace <bundle-id>`
- **THEN** CLI output includes required startup health/state fields
- **AND** includes required startup failure history fields

#### Scenario: Return lexicographically ordered all-mode output

- **WHEN** operator invokes `agentmux list principals --namespace '*'`
- **THEN** CLI output contains `bundles[]` ordered lexicographically by
  `bundle.id`

### Requirement: List Sessions Fanout Behavior

In `--namespace '*'` mode, CLI SHALL perform adapter-owned fanout by querying
bundles in lexicographic order.
Relay all-bundle list requests are not used.

On first `authorization_forbidden` from a bundle query, CLI SHALL:

- stop fanout immediately,
- query no further bundles,
- return canonical non-aggregate error output.

#### Scenario: Fail fast on first all-mode authorization denial

- **WHEN** `--namespace '*'` fanout encounters first `authorization_forbidden`
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

In `--namespace '*'` mode, encountering unreachable non-home bundle SHALL fail
with `relay_unavailable` and terminate fanout.

Home-bundle fallback startup-failure fields
(`startup_failure_count`, `recent_startup_failures`) SHALL be treated as
best-effort synthesized values from available local runtime state. When local
runtime failure history is unavailable, CLI SHALL return:

- `startup_failure_count=0`
- `recent_startup_failures=[]`

#### Scenario: Synthesize canonical home-bundle payload when relay is unreachable

- **WHEN** operator requests associated/home bundle principal listing
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

### Requirement: CLI raww command surface

CLI SHALL provide direct-write command:

`agentmux raww <target-session> --text <text> [--no-enter] [--bundle <name>] [--as-session <id>] [--json]`

`<target-session>` SHALL be canonical session id.
`--no-enter` default SHALL be `false`.

#### Scenario: Reject missing raww text

- **WHEN** operator invokes `agentmux raww` without `--text`
- **THEN** CLI rejects invocation with `validation_invalid_params`

#### Scenario: Map no-enter to no_enter true

- **WHEN** operator invokes `agentmux raww` with `--no-enter`
- **THEN** CLI forwards relay request with `no_enter = true`

### Requirement: CLI raww actor identity resolution

CLI raww acting identity SHALL follow global TUI-session selector contract:
- explicit `--as-session`
- otherwise configured default session in `tui.toml`

CLI SHALL NOT use repository association fallback for raww actor identity.

#### Scenario: Reject unknown as-session selector for raww

- **WHEN** operator passes unknown `--as-session` for `agentmux raww`
- **THEN** CLI rejects invocation with `validation_unknown_sender`

### Requirement: CLI raww relay taxonomy passthrough

CLI raww SHALL surface canonical relay validation/authorization codes unchanged,
including:
- `validation_unknown_target`
- `validation_cross_bundle_unsupported`
- `validation_invalid_params`
- `authorization_forbidden`

#### Scenario: Surface unknown target code for raww

- **WHEN** relay returns `validation_unknown_target` for raww
- **THEN** CLI surfaces `validation_unknown_target`

### Requirement: CLI raww machine output contract

When `--json` is requested, CLI raww successful output SHALL include required
fields:
- `status` (value `queued`)
- `target_session`
- `transport`

CLI MAY include optional fields:
- `request_id`
- `message_id`

#### Scenario: Return queued status in json output

- **WHEN** relay raww dispatch succeeds
- **THEN** CLI `--json` output includes `status = "queued"` with required fields

### Requirement: Relay Host Relay Configuration Controls

The `agentmux host relay` subcommand SHALL resolve relay-wide runtime controls
from CLI overrides, environment overrides, `<config-root>/relay.toml`, and
documented defaults, in that order. The `--no-watch` and
`--require-credentials` flags SHALL remain supported as CLI overrides for the
configuration file rather than as the durable source of truth.

When runtime configuration resolves `watch-bundles = false`, the relay SHALL NOT
start the bundle file watcher and SHALL ignore all filesystem changes to the
bundles configuration directory for the lifetime of that process. When the
setting resolves absent or `true`, watching is enabled.

When runtime configuration resolves `require-session-credentials = true`, the
relay SHALL enforce recognized session credentials on Hello. When the key is
absent or `false`, socket-trusted session connections remain allowed.

#### Scenario: Default configuration enables watching

- **WHEN** `agentmux host relay` is executed
- **AND** no CLI, environment, or `relay.toml` watch setting is supplied
- **THEN** relay starts the bundle file watcher after initial bundle load
  completes

#### Scenario: Relay configuration disables watcher

- **WHEN** `agentmux host relay` is executed
- **AND** `relay.toml` sets `watch-bundles = false`
- **THEN** relay starts without spawning a bundle file watcher
- **AND** filesystem changes to the bundles directory have no effect until relay
  restart

#### Scenario: Watch CLI override disables watcher

- **WHEN** an operator runs `agentmux host relay --no-watch`
- **THEN** relay starts without spawning a bundle file watcher

#### Scenario: Credential CLI override enforces credentials

- **WHEN** an operator runs `agentmux host relay --require-credentials`
- **THEN** relay enforces recognized session credentials on Hello

### Requirement: Configuration Pre-flight Command Surface

The system SHALL provide an `agentmux check configuration [<bundle-id>]`
subcommand that validates bundle configuration through the same loading path the
relay uses at startup, without starting the relay or mutating configuration.

The command SHALL accept an optional positional `<bundle-id>`: when present it
validates that single bundle; when omitted it validates every bundle in the
effective bundle set, which is the union of the bundles directories across every
configuration layer, with an entry in an earlier layer shadowing an entry of the
same identifier in a later one.

The command SHALL inherit the global runtime flags
(`--configuration-directory`, `--state-directory`,
`--inscriptions-directory`/`--logs-directory`), including the repeatability of
`--configuration-directory`.

The command SHALL additionally accept `-q`/`--quiet`, which suppresses all
success output — source reporting, per-bundle confirmations, and the summary —
leaving the exit code and any failure report. The flag is scoped to this
command; it is not a global runtime flag.

Validation SHALL cover bundle and coders schema and authorization-policy
resolution (`policies.toml`, `relay.toml`, and `users.toml` policy mappings),
matching what the relay rejects at startup, and SHALL resolve every file through
the shared effective-file lookup so it validates what the relay would actually
load.

The command SHALL be read-only: it MUST NOT scaffold or modify configuration
artifacts.

The command SHALL report, for each configuration artifact it resolves, the
physical file the effective lookup selected, so an operator can see which layer
supplied it. This SHALL be reported whether or not validation succeeds: a
shadowed file may be present, valid, and entirely inert, and no other surface
exposes which copy of an artifact is in effect.

Source reporting SHALL be default output rather than requested by a flag. The
operator who needs it is the one whose edit did nothing, and who therefore has
no reason to suspect a flag exists; putting it behind one would hide the
diagnosis behind already knowing the diagnosis exists. `--quiet` serves the
opposite need.

Reporting SHALL cover only artifacts a layer actually supplies. An artifact
absent from every layer contributes no line, which bounds the output to what
the deployment has; shadowing concerns a file that is present and inert, so
nothing diagnostic is lost.

Source reporting SHALL be emitted before validation runs. Validation is
fail-fast, so reporting interleaved with it would stop at the first invalid
artifact — on precisely the run where the full picture is most wanted.
Resolving sources is a lookup that cannot fail, so it can complete first.

Source reporting SHALL be written to standard output and failure reports to
standard error. Standard output SHALL be flushed before any failure report is
written, so that where the two streams share a destination — the merged
transcript an operator pastes into a bug report — the failure cannot appear
ahead of the source report explaining it. Streams captured separately carry no
ordering between them and the flush does not supply one.

The flush is redundant under a runtime that line-buffers standard output
regardless of destination, which is the present behavior. It is required because
that is an implementation choice rather than a guarantee: a standard output
buffered by destination would reorder a merged transcript with no other signal.

On success the command SHALL exit zero. On the first invalid bundle it SHALL
exit non-zero and report the offending file path and field-level detail; it does
not partially load or degrade gracefully. The reported path SHALL be the
physical file selected by the effective lookup, so an operator can tell which
layer is at fault. With an arbitrary number of layers the physical path is the
only way to identify the copy in effect, so reporting it is load-bearing rather
than a convenience.

#### Scenario: Validate a single named bundle

- **WHEN** an operator runs `agentmux check configuration <bundle-id>` against a
  valid configuration
- **THEN** the command exits zero
- **AND** reports the bundle as validated

#### Scenario: Validate every bundle in the effective set

- **WHEN** an operator runs `agentmux check configuration` with no positional
  argument
- **THEN** validation covers the union of bundle definitions across every layer
- **AND** a definition in an earlier layer shadows one of the same identifier in
  a later layer
- **AND** exits zero when all are valid

#### Scenario: Report an unknown configuration field

- **WHEN** a bundle file contains an unknown field (for example a misspelled
  session key)
- **THEN** the command exits non-zero
- **AND** reports the offending file path and the offending field

#### Scenario: Report the physical file at fault

- **WHEN** validation fails on a bundle supplied by a layer other than the last
- **THEN** the reported path is the file in that layer rather than any copy it
  shadows

#### Scenario: Reject an unknown check subcommand

- **WHEN** an operator runs `agentmux check <other>` where `<other>` is not
  `configuration`
- **THEN** the command rejects the invocation with a structured argument
  validation error

#### Scenario: Report when no bundles are discoverable

- **WHEN** an operator runs `agentmux check configuration` with no positional
  argument and no bundle files exist in any layer
- **THEN** the command exits non-zero
- **AND** reports that no bundle configurations were found

#### Scenario: Remain read-only

- **WHEN** the command runs against a configuration layer missing starter files
- **THEN** no configuration artifact is created or modified

#### Scenario: Report which layer supplied each artifact

- **WHEN** an operator runs `agentmux check configuration` against a valid
  multi-layer configuration
- **THEN** each resolved artifact is reported on standard output with the
  physical file that supplied it
- **AND** a copy shadowed by an earlier layer is distinguishable from the copy
  in effect

#### Scenario: Report sources before a validation failure

- **WHEN** validation fails against a multi-layer configuration
- **THEN** the source report for every resolved artifact precedes the failure
  report in a transcript capturing both streams

#### Scenario: Suppress success output under quiet

- **WHEN** an operator runs `agentmux check configuration --quiet` against a
  valid configuration
- **THEN** no source report, per-bundle confirmation, or summary is written
- **AND** the command exits zero

#### Scenario: Quiet still reports a failure

- **WHEN** an operator runs `agentmux check configuration --quiet` against an
  invalid configuration
- **THEN** the failure is reported
- **AND** the command exits non-zero

### Requirement: Configuration Root Command-Line Surface

The global runtime flag selecting configuration roots SHALL be named
`--configuration-directory`. It SHALL be honored identically in every build
profile.

The flag SHALL be accepted repeatably. Each occurrence appends one configuration
layer, and the layers are searched in the order given, so the first occurrence
is the highest-precedence layer. Help text for the flag SHALL state which end of
the list wins.

An occurrence with an empty value SHALL be rejected with a structured validation
error rather than contributing a layer.

`--discover-local-configuration` SHALL NOT be accepted. Ancestor-based discovery
located a configuration root inside the project being worked on, and
configuration no longer lives there; an explicit layer names the target instead
of inferring it.

#### Scenario: Select a configuration layer in any build profile

- **WHEN** an operator passes `--configuration-directory <path>`
- **THEN** the layer list is that single path
- **AND** the behavior is identical in debug and release builds

#### Scenario: Repeat the flag to declare layer order

- **WHEN** an operator passes `--configuration-directory A` then
  `--configuration-directory B`
- **THEN** the layer list is `[A, B]`
- **AND** a file present in both resolves from `A`

#### Scenario: Reject an empty flag value

- **WHEN** an operator passes `--configuration-directory` with an empty value
- **THEN** the command returns a structured validation error
- **AND** no layer is contributed

#### Scenario: Accept a relative configuration directory

- **WHEN** an operator passes a relative `--configuration-directory`
- **THEN** it resolves against the current working directory

### Requirement: Default Bundle Selector for MCP Hosting

`agentmux host mcp` SHALL accept an optional `--default-bundle <name>` that
supplies a bundle in the default tier of association resolution, distinct from
`--bundle`, which asserts invocation intent and outranks the injected bring-up
environment.

This allows generated client configuration to seed a bundle without overriding
what bring-up authoritatively knows.

#### Scenario: Default bundle yields to injected environment

- **WHEN** `agentmux host mcp --default-bundle alpha` is invoked
- **AND** the injected bring-up environment names bundle `beta`
- **THEN** bundle association resolves to `beta`

#### Scenario: Explicit bundle outranks injected environment

- **WHEN** `agentmux host mcp --bundle alpha` is invoked
- **AND** the injected bring-up environment names bundle `beta`
- **THEN** bundle association resolves to `alpha`

#### Scenario: Default bundle applies when no higher tier resolves

- **WHEN** `agentmux host mcp --default-bundle alpha` is invoked
- **AND** no explicit or injected bundle is present
- **AND** the effective association file supplies none
- **THEN** bundle association resolves to `alpha`

### Requirement: Deferred Argument Validation for MCP Hosting

Invalid arguments SHALL NOT fail process startup once `host mcp` is identifiable
as the requested command. The fault SHALL be retained and reported at
tool-invocation time, and no partially parsed argument value SHALL be used.

Other subcommands SHALL retain immediate argument validation, because they are
invoked by operators at a shell rather than by an MCP client.

#### Scenario: Invalid MCP argument does not erase the tool surface

- **WHEN** `agentmux host mcp` is invoked with an unrecognized flag
- **THEN** the process starts and advertises its tools
- **AND** the argument fault is reported on tool invocation

#### Scenario: Partially parsed arguments are not used

- **WHEN** `agentmux host mcp` receives a malformed value for a recognized flag
- **THEN** no value is derived from the malformed input
- **AND** the fault is retained

#### Scenario: Other subcommands still reject invalid arguments immediately

- **WHEN** an operator runs a non-`host mcp` subcommand with an unrecognized flag
- **THEN** the command exits non-zero with an argument error

