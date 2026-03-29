# runtime-bootstrap Specification

## Purpose
TBD - created by archiving change add-runtime-bootstrap-and-xdg-layout. Update Purpose after archive.
## Requirements
### Requirement: XDG Configuration Root

The system SHALL resolve default configuration root as:

- debug builds: repository-local
  `.auxiliary/configuration/agentmux/` when that directory exists
- otherwise: `$XDG_CONFIG_HOME/agentmux` or `~/.config/agentmux`

Explicit configuration path overrides (CLI or local override file fields) SHALL
continue to take precedence over default file resolution.

#### Scenario: Use repository-local config root in debug build

- **WHEN** runtime is debug/development mode
- **AND** `.auxiliary/configuration/agentmux/` exists under workspace root
- **AND** no explicit config path override is provided
- **THEN** bundle loading uses that repository-local config root

#### Scenario: Ignore repository-local file in release build

- **WHEN** runtime is non-debug/release mode
- **AND** `.auxiliary/configuration/agentmux/` exists
- **AND** no explicit config path override is provided
- **THEN** bundle loading uses XDG/home configuration resolution

#### Scenario: Explicit config override takes precedence

- **WHEN** runtime startup receives an explicit config path override
- **THEN** bundle loading uses the explicit path
- **AND** default debug/release config path logic is bypassed

### Requirement: XDG State Root

The system SHALL resolve the state root using:

- `$XDG_STATE_HOME/agentmux` when `XDG_STATE_HOME` is set and non-empty
- `~/.local/state/agentmux` otherwise

#### Scenario: Resolve state root from XDG variable

- **WHEN** `XDG_STATE_HOME` is set to a non-empty value
- **THEN** state root resolves under that directory

#### Scenario: Resolve state root from fallback

- **WHEN** `XDG_STATE_HOME` is unset or empty
- **THEN** state root resolves to `~/.local/state/agentmux`

### Requirement: Debug Repository-Local State Override

Debug builds SHALL support an optional repository-local state override to
isolate development runtime data from deployed runtime state.

#### Scenario: Use repository-local override in debug build

- **WHEN** runtime is debug/development mode
- **AND** repository-local override is configured
- **THEN** state root resolves to repository-local override path

#### Scenario: Ignore repository-local override in non-debug build

- **WHEN** runtime is not debug/development mode
- **THEN** state root resolution follows XDG state rules

### Requirement: Per-Bundle Runtime Layout

Each bundle SHALL use a dedicated runtime directory:

- `<state_root>/bundles/<bundle_name>/`

The system SHALL use:

- `<bundle_runtime>/tmux.sock`
- `<bundle_runtime>/relay.sock`

#### Scenario: Resolve per-bundle sockets

- **WHEN** runtime paths are resolved for a bundle
- **THEN** tmux operations use that bundle's `tmux.sock`
- **AND** MCP-to-relay IPC uses that bundle's `relay.sock`

### Requirement: Relay Connectivity Handling from MCP

MCP bootstrap SHALL resolve bundle and sender association at startup without
requiring relay connectivity.
Relay connectivity SHALL be checked when MCP tools invoke relay-backed
operations.
If connection fails, MCP tool responses SHALL return a structured
`relay_unavailable` error and MCP process startup SHALL remain successful.

#### Scenario: Fail startup before relay bootstrap when bundle is unknown

- **WHEN** bundle discovery resolves to an unknown or missing bundle
- **THEN** MCP startup returns structured `validation_unknown_bundle`
- **AND** relay connectivity checks are not required for startup

#### Scenario: Start MCP when relay is unavailable after association resolves

- **WHEN** bundle and sender association resolve successfully
- **AND** `relay.sock` is not connectable
- **THEN** MCP startup succeeds
- **AND** MCP does not attempt relay auto-spawn

#### Scenario: Return structured relay-unavailable error from tool call

- **WHEN** MCP receives a relay-backed tool request
- **AND** `relay.sock` is not connectable
- **THEN** MCP returns a structured `relay_unavailable` tool error

### Requirement: Relay Auto-Start Primitive for Non-MCP Clients

Runtime bootstrap helpers SHALL support optional relay auto-start for future
non-MCP clients such as TUI/CLI entrypoints.

Default bootstrap values SHALL be:

- `auto_start_relay = true`
- `startup_timeout_ms = 10000`

`agentmux tui` startup SHALL invoke this helper before entering the interactive
event loop.

When helper-triggered spawn is required for `agentmux tui`, spawned relay
invocation SHALL use the same resolved runtime roots as TUI startup:

- `--config-directory` from active runtime resolution
- `--state-directory` from active runtime resolution
- `--inscriptions-directory` from active runtime resolution

#### Scenario: Auto-start relay when unavailable

- **WHEN** bootstrap helper is called with `auto_start_relay = true`
- **AND** `relay.sock` is not connectable
- **THEN** helper executes relay spawn flow
- **AND** waits up to configured timeout for relay connectivity

#### Scenario: Fail fast when helper auto-start is disabled

- **WHEN** bootstrap helper is called with `auto_start_relay = false`
- **AND** `relay.sock` is not connectable
- **THEN** helper returns a structured bootstrap error

#### Scenario: Start tui with matching-root relay auto-spawn

- **WHEN** operator starts `agentmux tui`
- **AND** resolved `relay.sock` is unavailable
- **THEN** startup invokes relay auto-start helper
- **AND** helper spawn uses the same resolved `--config-directory`,
  `--state-directory`, and `--inscriptions-directory` values

### Requirement: TUI Auto-Spawn Relay Lifecycle Ownership

In MVP, relay auto-start from `agentmux tui` SHALL be bootstrap-only.

`agentmux tui` SHALL NOT terminate a relay process on TUI exit solely because
that relay was auto-started by that TUI invocation.

#### Scenario: Keep auto-started relay running after tui exit

- **WHEN** `agentmux tui` auto-starts relay
- **AND** TUI exits normally or via signal
- **THEN** relay process remains running until explicitly managed by relay
  lifecycle controls (`agentmux host relay`, service manager, or operator action)

### Requirement: Spawn Coordination and Stale Socket Handling

Relay startup SHALL use lock-based spawn coordination so exactly one contender
spawns relay while others wait for socket readiness.

#### Scenario: Single spawner under contention

- **WHEN** multiple clients invoke relay auto-start bootstrap concurrently for
  one bundle
- **THEN** only one process performs relay spawn
- **AND** other processes wait for relay socket connectability

#### Scenario: Remove stale relay socket before spawn

- **WHEN** relay socket exists but no live relay process holds runtime lock
- **THEN** bootstrap removes the stale socket before spawning relay

### Requirement: Sender Association Resolution

The MCP server SHALL resolve sender association at startup using precedence:

1. explicit CLI `--session-name` when present
2. local override file `session_name` when present
3. auto-discovered sender session

Auto-discovered sender session SHALL use:

- basename of Git worktree top-level directory when running inside Git
- otherwise basename of current working directory

#### Scenario: Resolve sender from worktree basename

- **WHEN** MCP starts inside a Git worktree rooted at
  `/home/me/src/WORKTREES/agentmux/relay`
- **AND** no CLI or override sender is provided
- **THEN** sender association resolves to `relay`

#### Scenario: Resolve sender from explicit CLI value

- **WHEN** MCP startup has explicit `--session-name`
- **THEN** sender association is set to that configured session

#### Scenario: Resolve sender from local override file

- **WHEN** CLI sender is absent
- **AND** local override file provides `session_name`
- **THEN** sender association is set to override value

#### Scenario: Reject ambiguous sender association

- **WHEN** sender association candidate matches multiple configured members
- **THEN** MCP startup returns a structured `validation_unknown_sender` error

### Requirement: Runtime Security Posture

Runtime artifacts SHALL remain inside same-user ownership and restrictive local
permissions.

#### Scenario: Create restrictive runtime directory

- **WHEN** system creates bundle runtime directory
- **THEN** directory mode is `0700`

#### Scenario: Reject foreign-owned runtime artifact

- **WHEN** an existing runtime socket or lock file is not owned by current user
- **THEN** bootstrap returns a structured security error

### Requirement: Startup Guidance for Shared Runtime Roots

Project documentation SHALL provide a recommended startup pattern where relay
starts before MCP, and relay/MCP use matching `--bundle` and
`--state-directory` values.

#### Scenario: Document startup order and shared state directory

- **WHEN** an operator configures local runtime startup
- **THEN** documented guidance specifies relay-first startup
- **AND** documented guidance specifies matching bundle and state-directory
  values across relay and MCP commands

### Requirement: Bundle Association Resolution

The MCP server SHALL resolve bundle association at startup using precedence:

1. explicit CLI `--bundle-name` when present
2. local override file `bundle_name` when present
3. auto-discovered bundle

Auto-discovered bundle SHALL use:

- basename of parent directory of Git common-dir when running inside Git
- otherwise basename of current working directory

Resolved bundle SHALL map to a configured bundle definition; otherwise startup
fails with structured `validation_unknown_bundle`.

#### Scenario: Resolve bundle from Git common-dir

- **WHEN** MCP starts in a Git worktree whose Git common-dir is
  `/home/me/src/agentmux/.git`
- **AND** no CLI or override bundle is provided
- **THEN** bundle association resolves to `agentmux`

#### Scenario: Resolve bundle from local override file

- **WHEN** CLI bundle is absent
- **AND** local override file provides `bundle_name`
- **THEN** bundle association resolves to override value

#### Scenario: Reject unknown bundle association

- **WHEN** resolved bundle has no corresponding configured bundle definition
- **THEN** MCP startup returns structured `validation_unknown_bundle`

#### Scenario: Resolve bundle from explicit CLI value

- **WHEN** MCP startup has explicit `--bundle-name`
- **THEN** bundle association is set to that configured bundle

### Requirement: Local MCP Association Override File

The MCP server SHALL support optional local association overrides in:

- `.auxiliary/configuration/agentmux/overrides/mcp.toml`

Supported override fields SHALL include:

- `bundle_name`
- `session_name`

The system MAY support optional config-root override fields for cross-project
bundle coordination.

#### Scenario: Ignore missing override file

- **WHEN** local override file does not exist
- **THEN** startup continues using CLI and auto-discovery resolution

#### Scenario: Reject malformed override file

- **WHEN** local override file exists but has invalid TOML or invalid fields
- **THEN** MCP startup returns a structured bootstrap validation error

### Requirement: Override Directory VCS Posture

The project SHALL Git-ignore the local override directory so overrides can be
used per worktree without leaking to shared commits.

#### Scenario: Ignore local override directory in Git

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/agentmux/overrides/` is ignored

### Requirement: Bundle Configuration File Name

Bundle configuration SHALL be stored as:

- `coders.toml`
- `bundles/<bundle-id>.toml`

Per-bundle `bundles/<bundle-name>.json` files SHALL NOT be required.

#### Scenario: Load bundle from per-bundle TOML plus coders TOML

- **WHEN** runtime resolves configuration defaults or explicit config path
- **THEN** bundle lookup reads `bundles/<bundle-id>.toml`
- **AND** coder lookup reads `coders.toml`

#### Scenario: Fail when bundle file is absent

- **WHEN** requested bundle ID does not have matching
  `bundles/<bundle-id>.toml`
- **THEN** startup returns structured `validation_unknown_bundle`

### Requirement: Bundle Group Resolution

Bundle group selector resolution SHALL apply to bundle lifecycle commands:

- `agentmux up --group <GROUP>`
- `agentmux down --group <GROUP>`

Group membership SHALL resolve from bundle-local configuration under:

- `<config-root>/bundles/<bundle-id>.toml`

Bundle files MAY define optional top-level:

- `groups` (`string[]`)

Group naming rules:

- reserved/system group names are uppercase
- custom group names are lowercase
- MVP reserved group `ALL` is implicit and selects all configured bundles

#### Scenario: Resolve custom group for bundle lifecycle command

- **WHEN** an operator invokes `agentmux up --group dev`
- **THEN** the system selects bundles whose `groups` include `dev`

#### Scenario: Resolve ALL as implicit group

- **WHEN** an operator invokes `agentmux down --group ALL`
- **THEN** the system selects all configured bundles
- **AND** does not require explicit `ALL` membership in bundle files

#### Scenario: Treat missing groups key as no custom group membership

- **WHEN** a bundle file omits `groups`
- **THEN** that bundle is still selectable by `<bundle-id>` and `--group ALL`
- **AND** it is not selected for custom groups unless explicitly listed

#### Scenario: Reject unknown custom group

- **WHEN** an operator invokes `agentmux up --group nightly`
- **AND** no configured bundle contains group `nightly`
- **THEN** the system rejects invocation with `validation_unknown_group`

#### Scenario: Reject invalid custom uppercase group name

- **WHEN** an operator invokes `agentmux down --group DEV`
- **AND** `DEV` is not a reserved system group
- **THEN** the system rejects invocation with `validation_invalid_group_name`

### Requirement: Relay Group Trust Boundary

Bundle lifecycle group operations SHALL remain within the existing local runtime
trust boundary:

- same-user ownership checks for runtime artifacts,
- same-host local socket assumptions,
- no new remote control surface.

#### Scenario: Enforce existing ownership checks for group-selected bundles

- **WHEN** `agentmux up --group dev` initializes runtime artifacts for selected
  bundles
- **THEN** ownership and permission checks remain enforced per bundle
- **AND** foreign-owned runtime artifacts are rejected

### Requirement: Persistent Relay Client Mode for MCP and TUI

MCP and TUI relay clients SHALL use persistent relay stream connections in MVP
rather than per-request reconnect behavior.

MCP and TUI clients SHALL perform `hello` registration on stream setup before
sending relay request frames.

`hello` registration in runtime clients SHALL use canonical routing identity:

- associated runtime `bundle_name`
- canonical `session_id`
- `client_class`

#### Scenario: MCP establishes persistent agent-class relay stream

- **WHEN** MCP performs first relay-backed operation in runtime
- **THEN** MCP establishes persistent relay stream
- **AND** registers with `hello` using associated `bundle_name`,
  canonical `session_id`, and `client_class=agent`

#### Scenario: TUI establishes persistent ui-class relay stream

- **WHEN** TUI starts and relay connectivity is available
- **THEN** TUI establishes persistent relay stream
- **AND** registers with `hello` using associated `bundle_name`,
  canonical `session_id`, and `client_class=ui`

### Requirement: Stream Reconnect Behavior

On stream disconnect, clients SHALL attempt reconnect with same identity and
repeat `hello` registration.

Reconnect failures SHALL be surfaced as `relay_unavailable` errors in existing
caller-facing paths.

Reconnect logic SHALL preserve identity-ownership hardening behavior:

- reconnect `hello` claim is accepted when no conflicting live owner exists for
  `(bundle_name, session_id)`, or when prior owner is already hard-dead per
  relay evidence contract;
- conflicting live-owner claims are rejected with
  `runtime_identity_claim_conflict`.

#### Scenario: Re-register identity after reconnect without live conflict

- **WHEN** client stream reconnect succeeds after disconnect
- **AND** no conflicting live owner exists for that identity
- **THEN** client sends `hello` with same identity
- **AND** relay accepts identity binding

#### Scenario: Reject reconnect claim while prior owner remains live

- **WHEN** reconnect attempt sends `hello` for identity with conflicting live
  owner
- **THEN** relay rejects claim with `runtime_identity_claim_conflict`

#### Scenario: Surface relay unavailable on reconnect failure

- **WHEN** reconnect attempt fails to establish stream
- **THEN** client surfaces `relay_unavailable` in caller-facing error path

### Requirement: TUI Sender Association Resolution

The TUI runtime SHALL resolve sender association at startup using precedence:

1. explicit CLI `--sender` when present
2. local testing override sender from
   `.auxiliary/configuration/agentmux/overrides/tui.toml` when present in
   debug/testing mode
3. normal config sender from `<config-root>/tui.toml` when present
4. runtime association auto-discovery

If sender cannot be resolved or does not map to a known bundle member, startup
SHALL fail with structured `validation_unknown_sender`.

#### Scenario: Resolve sender from CLI override

- **WHEN** TUI startup includes explicit `--sender`
- **THEN** sender association is set to that configured session

#### Scenario: Resolve sender from debug/testing override

- **WHEN** CLI sender is absent
- **AND** runtime is debug/testing mode
- **AND** `overrides/tui.toml` provides `sender`
- **THEN** sender association resolves from override sender value

#### Scenario: Resolve sender from normal config tui.toml

- **WHEN** CLI sender is absent
- **AND** override sender is absent or not active for current mode
- **AND** `<config-root>/tui.toml` provides `sender`
- **THEN** sender association resolves from normal config sender value

#### Scenario: Resolve sender from runtime association fallback

- **WHEN** all configured sender files are absent or inapplicable
- **THEN** runtime association fallback is used to resolve sender identity

#### Scenario: Reject unresolved sender association

- **WHEN** all sender resolution sources fail to produce a valid sender
- **THEN** TUI startup returns structured `validation_unknown_sender`

### Requirement: TUI Sender Configuration Files

The TUI runtime SHALL support optional sender configuration files:

- normal config path: `<config-root>/tui.toml`
- debug/testing override path:
  `.auxiliary/configuration/agentmux/overrides/tui.toml`

Supported fields for this proposal SHALL include:

- `sender`

Missing files SHALL not be treated as errors.
Malformed files SHALL fail fast with structured bootstrap validation errors.

#### Scenario: Ignore missing tui sender config files

- **WHEN** both normal and override `tui.toml` are missing
- **THEN** startup continues using remaining precedence sources

#### Scenario: Reject malformed normal tui.toml

- **WHEN** `<config-root>/tui.toml` exists but has invalid TOML or invalid
  fields
- **THEN** startup fails with structured bootstrap validation error

#### Scenario: Reject malformed override tui.toml in debug/testing mode

- **WHEN** override `tui.toml` is active for mode and malformed
- **THEN** startup fails with structured bootstrap validation error

### Requirement: TUI Override File VCS Posture

TUI local testing override file SHALL follow the existing local override VCS
posture so per-user test defaults do not leak into shared tracked
configuration.

#### Scenario: Keep override tui.toml under ignored overrides directory

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/agentmux/overrides/tui.toml` is covered by
  the existing ignored overrides path

### Requirement: Bundle Autostart Eligibility Field

Per-bundle TOML configuration SHALL support optional top-level:

- `autostart` (boolean)

If omitted, `autostart` SHALL default to `false`.

`autostart` SHALL only affect no-selector `agentmux host relay` autostart mode.

#### Scenario: Treat omitted autostart as false

- **WHEN** bundle file omits `autostart`
- **THEN** runtime resolves `autostart=false` for that bundle

#### Scenario: Resolve explicit autostart true

- **WHEN** bundle file sets `autostart = true`
- **THEN** runtime marks bundle as eligible for host autostart mode

### Requirement: Host Relay No-Selector Autostart Resolution

When operator runs `agentmux host relay` with no selector mode, runtime SHALL:

1. start relay process,
2. select bundles with `autostart=true`,
3. attempt hosting selected bundles using existing per-bundle host semantics.

When operator runs `agentmux host relay --no-autostart`, runtime SHALL start
relay process and SHALL skip bundle hosting selection.

No-selector mode success SHALL be based on relay process startup success and
SHALL NOT fail solely because zero bundles were selected/hosted.

#### Scenario: Start relay and host eligible bundles in no-selector mode

- **WHEN** operator runs `agentmux host relay`
- **THEN** runtime starts relay process
- **AND** selects bundles where `autostart=true`
- **AND** attempts hosting those bundles

#### Scenario: Start relay without bundle hosting in no-autostart mode

- **WHEN** operator runs `agentmux host relay --no-autostart`
- **THEN** runtime starts relay process
- **AND** does not perform bundle hosting selection

#### Scenario: Return success for no-selector mode with zero eligible bundles

- **WHEN** operator runs `agentmux host relay`
- **AND** no configured bundles have `autostart=true`
- **THEN** runtime returns successful process startup

### Requirement: Bundle Lifecycle Selector Resolution for Up and Down

`agentmux up` and `agentmux down` selector resolution SHALL follow existing
bundle/group selector semantics:

- positional `<bundle-id>` selects one configured bundle
- `--group <GROUP>` selects bundles by group membership (`ALL` implicit)

Unknown selectors SHALL return existing validation errors:

- `validation_unknown_bundle`
- `validation_unknown_group`
- `validation_invalid_group_name`

#### Scenario: Resolve up selector by bundle id

- **WHEN** operator runs `agentmux up relay`
- **THEN** runtime resolves one configured bundle named `relay`

#### Scenario: Reject down selector for unknown custom group

- **WHEN** operator runs `agentmux down --group nightly`
- **AND** no configured bundle declares group `nightly`
- **THEN** runtime returns `validation_unknown_group`
