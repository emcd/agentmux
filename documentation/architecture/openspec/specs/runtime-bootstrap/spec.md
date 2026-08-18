# runtime-bootstrap Specification

## Purpose
Runtime layout, configuration root resolution, and startup sequencing for the relay and MCP binaries. The spec governs configuration root resolution (explicit flag, environment, opt-in ancestor discovery, then the XDG default, identically in every build profile) and the overlay through which every configuration file resolves; state root resolution (explicit flag, environment, then the XDG and home defaults, identically in every build profile) and its propagation to spawned members; the relay-level and per-bundle runtime directory structure (`<state_root>/relay.sock`, `<bundle_runtime>/tmux.sock`, `<bundle_runtime>/sessions/<session>/identity.psk`); and sender and bundle association resolution precedence for MCP startup, which retains an unresolvable association as a reportable fault rather than failing to start. It also covers MCP-to-relay connectivity handling with `relay_unavailable` tool errors, the relay auto-start helper for `agentmux tui`, and the runtime security posture (per-user ownership, 0700 bundle directories, rejection of foreign-owned runtime artifacts).
## Requirements
### Requirement: XDG Configuration Root

The system SHALL resolve configuration from an ordered list of configuration
roots, called **layers**, using precedence:

1. explicit CLI `--configuration-directory`, accepted repeatably, each
   occurrence appending one layer in the order given
2. `AGENTMUX_CONFIGURATION_DIRECTORY` when set and non-blank, parsed as a
   `:`-separated list in the same order, each element resolved against the
   working directory when relative, identically to the CLI flag
3. `$XDG_CONFIG_HOME/agentmux` when set and non-empty, otherwise
   `~/.config/agentmux`, as a single layer

Tiers 1 and 2 SHALL **replace** the layer list rather than extend it, and a
supplied list SHALL be closed: no root outside the supplied list SHALL be
consulted for any file.

Closedness governs which roots are searched, not what absence means. A file
absent from every supplied layer is absent, and each artifact's existing
absence semantics continue to apply unchanged: an optional artifact such as
`mcp.toml`, `users.toml`, or `ui.toml` remains optional, while an artifact a
command requires still faults.

Absence SHALL mean that nothing exists at the path a layer would supply the file
from. That SHALL be the only condition resolution treats as a layer not
supplying a file. A layer whose contents cannot be determined — because the path
or an ancestor denies permission, because an ancestor is not a directory, or
because the read fails for any other reason — SHALL NOT be treated as a layer
that does not supply the file, and neither SHALL a layer holding something other
than a regular file at that path. Resolution SHALL report a fault naming the
physical path and the underlying cause, and SHALL NOT continue into later layers
on the strength of that failure. Falling through would substitute a
lower-precedence layer's value for the one an operator authored, and the
substitution is indistinguishable from correct resolution: the higher layer
exists precisely to shadow the lower one, so the observable result of losing it
is the base layer's value. This holds whether the higher layer is unreadable or
merely occupied by the wrong kind of file, since the two are distinguishable
only to someone already inspecting the layer and produce one symptom between
them.

This distinction SHALL apply to bundle-directory enumeration on the same terms,
at every point enumeration reads the filesystem. A layer with no `bundles/`
directory contributes no definitions, which is the ordinary case for a layer
that overrides only root-level artifacts. A `bundles/` directory which exists
and cannot be read SHALL fault rather than contribute an empty set. A failure
encountered while reading individual directory entries SHALL likewise fault
rather than truncate that layer's contribution, since a partial enumeration
reports success while omitting definitions the layer supplies.

Faulting on an unreadable layer SHALL NOT be interpreted as a requirement that
every consumer terminate. The distinction the system SHALL preserve is between
absence and failure; how a given surface responds to a reported failure belongs
to that surface. Startup and configuration load SHALL fault. The configuration
report SHALL render the unreadable layer as a finding and continue to its
remaining checks, since a diagnostic surface that aborts on the condition it
exists to diagnose reports nothing, and SHALL then fail rather than report the
configuration valid. A running relay's configuration watcher SHALL retain its
last successful reconciliation rather than terminate, since an unreadable layer
is recoverable and terminating converts a configuration fault into an outage.

Every element of a supplied list SHALL be a non-empty path. An empty element —
whether from a repeated flag with an empty value, or from a leading, trailing,
or doubled separator in the environment form — SHALL be rejected with a
structured validation error. An empty element SHALL NOT be interpreted as the
working directory, which would silently admit configuration from wherever a
process happened to be started.

The list SHALL be searched front to back, so the first layer is the
highest-precedence layer and the last is the base.

A layer SHALL be an ordinary configuration root. No subdirectory beneath a layer
SHALL be given special resolution meaning.

Configuration root resolution SHALL NOT depend on build profile.

#### Scenario: Resolve a single layer from an explicit CLI value

- **WHEN** startup receives one `--configuration-directory`
- **THEN** the layer list is that one path
- **AND** XDG/home resolution is bypassed

#### Scenario: Repeated flags append layers in order

- **WHEN** startup receives `--configuration-directory A` then
  `--configuration-directory B`
- **THEN** the layer list is `[A, B]`
- **AND** a file present in both resolves from `A`

#### Scenario: Resolve layers from environment

- **WHEN** no `--configuration-directory` is provided
- **AND** `AGENTMUX_CONFIGURATION_DIRECTORY` is set to `A:B`
- **THEN** the layer list is `[A, B]`

#### Scenario: Supplied layers do not fall through for undefined files

- **WHEN** a layer list is supplied explicitly
- **AND** a requested configuration file exists under none of its layers
- **THEN** resolution reports the file as absent
- **AND** no unsupplied configuration root is consulted

#### Scenario: Absence keeps each artifact's own semantics

- **WHEN** an optional artifact exists under none of the supplied layers
- **THEN** the command proceeds as it would with a single root

#### Scenario: An unreadable earlier layer faults rather than falling through

- **WHEN** the layer list is `[A, B]`
- **AND** a requested configuration file exists under both `A` and `B`
- **AND** `A` cannot be read because it denies permission
- **THEN** resolution reports a fault naming `A` and the underlying cause
- **AND** the value from `B` is not used

#### Scenario: An unreadable layer faults for an optional artifact

- **WHEN** the layer list is `[A, B]`
- **AND** an optional artifact is absent from `B`
- **AND** `A` cannot be read because it denies permission
- **THEN** resolution reports a fault rather than reporting the artifact absent

#### Scenario: A non-file occupying an artifact path faults

- **WHEN** the layer list is `[A, B]`
- **AND** a requested configuration file exists under `B`
- **AND** `A` holds a directory at that file's path
- **THEN** resolution reports a fault naming the path in `A`
- **AND** the value from `B` is not used

#### Scenario: A layer without a bundles directory contributes nothing

- **WHEN** the layer list is `[A, B]`
- **AND** `A` has no `bundles/` directory
- **THEN** enumeration succeeds with the definitions supplied by `B`

#### Scenario: An unreadable bundles directory faults

- **WHEN** a layer's `bundles/` directory exists and cannot be read
- **THEN** enumeration reports a fault naming that directory and the underlying
  cause
- **AND** the layer does not contribute an empty set of definitions

#### Scenario: A bundle-shaped directory entry faults

- **WHEN** a layer's `bundles/` directory holds a directory named
  `<identifier>.toml`
- **THEN** enumeration reports a fault naming that path
- **AND** the identifier does not resolve from a later layer as though the
  earlier layer defined nothing

#### Scenario: An unrelated entry under a bundles directory is ignored

- **WHEN** a layer's `bundles/` directory holds an entry whose name does not end
  in the bundle extension
- **THEN** enumeration ignores it whatever its type
- **AND** enumeration succeeds

#### Scenario: The configuration report renders an unreadable layer

- **WHEN** `check configuration` runs against a layer list containing an
  unreadable layer
- **THEN** the report names the unreadable layer and the underlying cause
- **AND** the command does not abort before rendering its remaining findings
- **AND** the command fails, rather than reporting the configuration valid

#### Scenario: A running relay survives an unreadable layer

- **WHEN** a relay's configuration watcher observes a layer that has become
  unreadable
- **THEN** the relay retains its last successful reconciliation
- **AND** the relay does not terminate

#### Scenario: Reject an empty layer element

- **WHEN** a supplied layer list contains an empty element, from an empty flag
  value or from a leading, trailing, or doubled separator in the environment
  form
- **THEN** startup returns a structured validation error naming the offending
  position
- **AND** the working directory is not consulted as a configuration root

#### Scenario: Resolve configuration root from XDG default

- **WHEN** no explicit layer list is provided
- **THEN** the layer list is the single root `$XDG_CONFIG_HOME/agentmux` or
  `~/.config/agentmux`

#### Scenario: Configuration root resolution is identical across build profiles

- **WHEN** the same inputs are supplied to a debug build and a release build
- **THEN** both resolve the same layer list

### Requirement: XDG State Root

The system SHALL resolve the state root using, in order:

- the explicit `--state-directory` value when supplied
- `AGENTMUX_STATE_DIRECTORY` when set and non-empty
- `$XDG_STATE_HOME/agentmux` when `XDG_STATE_HOME` is set and non-empty
- `~/.local/state/agentmux` otherwise

The resolved state root SHALL be identical in every build profile. Build profile
SHALL NOT influence state or inscriptions root resolution.

The resolved state root SHALL be normalized to a non-empty absolute path before
use, resolving a relative value against the working directory of the process
performing the resolution. An unnormalized root cannot be propagated: a relative
path re-resolves against each spawned process's working directory, silently
sending a child to a different state root than the relay that spawned it.

`--state-directory` with an empty value SHALL be rejected with a structured
validation error. Empty is not the same signal as absent here: the environment
tier treats blank as absent, so accepting an empty flag would give one spelling
of "nothing" two different meanings depending on which surface carried it.

One state root SHALL correspond to one relay. Isolating a deployment SHALL be
expressed by naming a distinct state root; no deployment identifier is derived
from configuration, build profile, or repository location.

The inscriptions root SHALL continue to default to `<state_root>/inscriptions`,
and therefore follows the state root without separate selection.

A blank `AGENTMUX_STATE_DIRECTORY` SHALL be treated as absent, matching every
other environment tier.

#### Scenario: Resolve state root from XDG variable

- **WHEN** `XDG_STATE_HOME` is set to a non-empty value
- **AND** no explicit or environment state directory is supplied
- **THEN** state root resolves under that directory

#### Scenario: Resolve state root from fallback

- **WHEN** `XDG_STATE_HOME` is unset or empty
- **AND** no explicit or environment state directory is supplied
- **THEN** state root resolves to `~/.local/state/agentmux`

#### Scenario: Environment tier selects the state root

- **WHEN** `AGENTMUX_STATE_DIRECTORY` is set to a non-empty value
- **THEN** state root resolves to that path
- **AND** the XDG and home defaults are not consulted

#### Scenario: Explicit flag outranks the environment tier

- **WHEN** an operator passes `--state-directory`
- **AND** `AGENTMUX_STATE_DIRECTORY` is also set
- **THEN** the state root is the flag's value

#### Scenario: Reject an empty state directory

- **WHEN** an operator passes `--state-directory` with an empty value
- **THEN** the command returns a structured validation error

#### Scenario: Normalize a relative state root

- **WHEN** a relative state root is supplied by flag or environment
- **THEN** it resolves against the working directory into an absolute path
- **AND** that absolute path is what downstream resolution and propagation use

#### Scenario: Build profile does not change the state root

- **WHEN** a debug build and a release build resolve roots from identical
  arguments and environment
- **THEN** both resolve the same state root and the same inscriptions root

### Requirement: Runtime Layout

Relay-level artifacts SHALL live at the state-root level:

- `<state_root>/relay.sock`
- `<state_root>/relay.lock`
- `<state_root>/relay.spawn.lock`
- `<state_root>/relay.ready`
- `<state_root>/identity/` — relay-level identity subsystem directory
- `<state_root>/identity/principals.json` — durable principal store

Each bundle SHALL use a dedicated runtime directory for per-bundle artifacts:

- `<state_root>/bundles/<bundle_name>/`
- `<bundle_runtime>/tmux.sock`
- `<bundle_runtime>/sessions/<session>/identity.psk` — session credential file

#### Scenario: Resolve relay-level and per-bundle paths

- **WHEN** runtime paths are resolved
- **THEN** MCP-to-relay IPC uses the single `<state_root>/relay.sock`
- **AND** tmux operations use the per-bundle `<bundle_runtime>/tmux.sock`

#### Scenario: Resolve relay-level principal store

- **WHEN** the relay initializes or processes a Hello
- **THEN** the principal store is located at `<state_root>/identity/principals.json`
- **AND** the `<state_root>/identity/` directory is created if it does not exist

#### Scenario: Resolve session credential file

- **WHEN** a session client reads its identity token
- **THEN** the token is loaded from `<bundle_runtime>/sessions/<session>/identity.psk`

### Requirement: Relay Connectivity Handling from MCP

MCP bootstrap SHALL NOT require relay connectivity.
Relay connectivity SHALL be checked when MCP tools invoke relay-backed
operations.
If connection fails, MCP tool responses SHALL return a structured
`relay_unavailable` error and MCP process startup SHALL remain successful.

One exception is retained unchanged: the MCP list surface MAY return its
synthetic home-bundle payload when the relay is unreachable, rather than
`relay_unavailable`, as specified by `mcp-tool-surface`. That fallback is
distinct from a retained startup fault and is not affected by startup fault
tolerance.

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

- `--configuration-directory` from active runtime resolution
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
- **AND** helper spawn uses the same resolved `--configuration-directory`,
  `--state-directory`, and `--inscriptions-directory` values

### Requirement: TUI Auto-Spawn Relay Lifecycle Ownership

Relay auto-start from `agentmux tui` SHALL establish TUI ownership of the relay
it spawns.

`agentmux tui` SHALL terminate a relay process on TUI exit if and only if that
relay was auto-spawned by that same TUI invocation. A relay that was already
running when the TUI started (service manager or operator action) SHALL NOT be
terminated on TUI exit.

The auto-spawned relay SHALL be started in its own process group so its
lifecycle is governed solely by the TUI's explicit termination signal, not by
incidental terminal signal propagation.

Termination SHALL reuse the relay's standard signal-driven shutdown path, which
prunes the tmux sessions the relay owns and reaps the tmux server when it becomes
unowned. A TUI-auto-spawned relay is therefore an ad hoc single-operator
convenience, not a durable shared relay.

#### Scenario: Stop auto-spawned relay on tui exit

- **WHEN** `agentmux tui` auto-spawns a relay because none was reachable
- **AND** the TUI later exits normally or via signal
- **THEN** the TUI sends the spawned relay a graceful termination signal
- **AND** relay shutdown prunes the tmux sessions it owns and reaps the tmux
  server when it becomes unowned

#### Scenario: Leave already-running relay after tui exit

- **WHEN** a relay is already reachable when `agentmux tui` starts
- **AND** the TUI does not auto-spawn a relay
- **AND** the TUI later exits
- **THEN** the TUI does not signal or terminate that relay
- **AND** the relay remains running under its existing lifecycle controls
  (`agentmux host relay`, service manager, or operator action)

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
2. injected bring-up environment variable `AGENTMUX_SESSION` when present and
   non-blank
3. the effective association file's `session_name` when present
4. working-directory match against configured member directories

A blank injected value SHALL be treated as absent.

A tier SHALL apply only when every tier above it is absent. When a tier supplies
a sender that names no configured member, sender association SHALL be recorded as
unresolved with that cause and SHALL NOT fall through to a lower tier.

Sender association SHALL NOT be derived from Git metadata. When no tier supplies
a sender and no configured member matches, sender association SHALL be recorded
as unresolved rather than failing startup.

The tier which supplied the resolved sender SHALL be recorded. The association
file occupies one tier regardless of how many configuration layers were searched
to produce it.

#### Scenario: Resolve sender from explicit CLI value

- **WHEN** MCP startup has explicit `--session-name`
- **THEN** sender association is set to that configured session

#### Scenario: Injected environment wins over association file

- **WHEN** CLI sender is absent
- **AND** the `AGENTMUX_SESSION` environment value is present and non-blank
- **AND** the effective association file also provides `session_name`
- **THEN** sender association resolves to the environment value

#### Scenario: Resolve sender from working directory match

- **WHEN** CLI, environment, and association file senders are all absent
- **AND** the working directory matches a configured member's declared directory
- **THEN** sender association resolves to that member

#### Scenario: Blank injected sender is ignored

- **WHEN** `AGENTMUX_SESSION` is set to a blank value
- **THEN** it contributes no identity
- **AND** resolution continues with the next tier

#### Scenario: Supplied sender naming no member does not fall through

- **WHEN** a sender is supplied by CLI, environment, or the effective association
  file
- **AND** it names no configured member
- **AND** the working directory matches a different configured member
- **THEN** sender association is recorded as unresolved with that cause
- **AND** the working-directory match is not applied

#### Scenario: Record unresolved sender without failing startup

- **WHEN** no tier supplies a sender
- **AND** the working directory matches no configured member
- **THEN** sender association is recorded as unresolved with its cause
- **AND** MCP startup succeeds

#### Scenario: Record the tier which supplied the sender

- **WHEN** sender association resolves
- **THEN** the startup record names the tier it resolved from

#### Scenario: Reject ambiguous sender association

- **WHEN** the sender association candidate matches multiple configured members
- **THEN** sender association is recorded as unresolved with an ambiguity cause

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

1. explicit CLI `--bundle` when present
2. injected bring-up environment variable `AGENTMUX_BUNDLE` when present and
   non-blank
3. the effective association file's `bundle_name` when present
4. explicit CLI `--default-bundle` when present

A blank injected value SHALL be treated as absent.

`--default-bundle` SHALL occupy the default tier so generated client
configuration can seed a bundle without outranking bring-up, while `--bundle`
retains its meaning as an assertion of invocation intent.

Bundle association SHALL NOT be derived from Git metadata. When no tier supplies
a bundle, bundle association SHALL be recorded as unresolved rather than failing
startup.

A bundle supplied by any tier, `--default-bundle` included, carries operator
intent. When a supplied bundle cannot be loaded, MCP startup SHALL retain the
loading fault with its own cause rather than recording a generic unassociated
server.

The tier which supplied the resolved bundle SHALL be recorded. The association
file occupies one tier regardless of how many configuration layers were searched
to produce it.

#### Scenario: Resolve bundle from explicit CLI value

- **WHEN** MCP startup has explicit `--bundle`
- **THEN** bundle association is set to that configured bundle

#### Scenario: Injected environment wins over default bundle

- **WHEN** `--bundle` is absent
- **AND** `--default-bundle` names one bundle
- **AND** the `AGENTMUX_BUNDLE` environment value names a different bundle
- **THEN** bundle association resolves to the environment value

#### Scenario: Injected environment wins over association file

- **WHEN** `--bundle` is absent
- **AND** the effective association file provides `bundle_name`
- **AND** the `AGENTMUX_BUNDLE` environment value is present and non-blank
- **THEN** bundle association resolves to the environment value

#### Scenario: Fall back to default bundle

- **WHEN** no CLI `--bundle`, environment, or association file value is present
- **AND** `--default-bundle` is provided
- **THEN** bundle association resolves to the default value

#### Scenario: Record unresolved bundle without failing startup

- **WHEN** no tier supplies a bundle
- **THEN** bundle association is recorded as unresolved with its cause
- **AND** MCP startup succeeds

#### Scenario: Retain the cause when a supplied bundle cannot be loaded

- **WHEN** a tier supplies a bundle which is unknown or malformed
- **THEN** MCP startup retains that loading fault with its own cause
- **AND** tool calls report that cause rather than a generic unassociated server

#### Scenario: Record the tier which supplied the bundle

- **WHEN** bundle association resolves
- **THEN** the startup record names the tier it resolved from

### Requirement: Local MCP Association Override File

The MCP server SHALL support optional association overrides in a logical
configuration artifact at relative path `mcp.toml`, resolved through the shared
effective-file lookup across the configuration layers.

The resolved artifact is the **effective association file**, and it occupies a
single tier in each association ladder regardless of how many layers were
searched to produce it.

Supported override fields SHALL be:

- `bundle_name`
- `session_name`

Fields SHALL be independently optional: a file supplying only one field SHALL
leave the other to the remaining association tiers.

The file SHALL NOT support a configuration-root field. A file located beneath a
configuration layer cannot redirect the layer list.

#### Scenario: Ignore missing association file

- **WHEN** `mcp.toml` exists under no configuration layer
- **THEN** startup continues using the remaining association tiers

#### Scenario: Nearest layer supplies the association file

- **WHEN** `mcp.toml` exists under more than one configuration layer
- **THEN** the copy from the earliest layer in the list is the effective
  association file
- **AND** copies in later layers contribute no fields

#### Scenario: Resolve bundle from the effective association file alone

- **WHEN** no CLI or injected bundle is present
- **AND** the effective association file supplies `bundle_name`
- **THEN** bundle association resolves to that value

#### Scenario: Resolve sender from the effective association file alone

- **WHEN** no CLI or injected sender is present
- **AND** the effective association file supplies `session_name`
- **THEN** sender association resolves to that value

#### Scenario: Apply one field and defer the other

- **WHEN** the effective association file supplies only `bundle_name`
- **THEN** bundle association uses that value
- **AND** sender association continues through its remaining tiers

#### Scenario: Reject malformed association file

- **WHEN** the effective association file has invalid TOML or unknown fields
- **THEN** the fault is recorded as a startup fault with its cause

### Requirement: Bundle Configuration File Name

Bundle configuration SHALL be stored as:

- `coders.toml`
- `bundles/<bundle-id>.toml`

Per-bundle `bundles/<bundle-name>.json` files SHALL NOT be required.

A command that resolves a requested bundle ID in order to act on it SHALL fail
when no matching effective bundle file exists. MCP startup SHALL retain that
condition instead, because it advertises its tool surface before any tool is
called.

#### Scenario: Load bundle from per-bundle TOML plus coders TOML

- **WHEN** runtime resolves configuration defaults or an explicit configuration
  root
- **THEN** bundle lookup reads the effective `bundles/<bundle-id>.toml`
- **AND** coder lookup reads the effective `coders.toml`

#### Scenario: Fail when bundle file is absent

- **WHEN** a command other than `host mcp` resolves a requested bundle ID with
  no matching effective `bundles/<bundle-id>.toml`
- **THEN** the command returns structured `validation_unknown_bundle`

#### Scenario: Retain an absent bundle file at MCP startup

- **WHEN** MCP startup resolves a requested bundle ID with no matching effective
  `bundles/<bundle-id>.toml`
- **THEN** the process starts and serves the protocol
- **AND** the cause is retained and reported on invocation of a tool requiring a
  resolved association, a loaded configuration, or relay access

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
- The reserved group `ALL` is implicit and selects all configured bundles

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

MCP and TUI relay clients SHALL use persistent relay stream connections
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

The runtime SHALL resolve sender identity for `agentmux tui` and
session-selected `agentmux send` invocations using global `users.toml`
identity configuration and `ui.toml` UI-surface defaults with deterministic
precedence.

Sender/session resolution SHALL be:

1. explicit CLI `--as-session` when present
2. `default-session` from active global `users.toml`
3. fail-fast `validation_unknown_session`

Bundle resolution for interactive `agentmux tui` SHALL be lenient — the operator
selects a browsing bundle in the picker, so an absent default is not an error:

1. explicit CLI `--bundle` when present
2. `default-bundle` from active `ui.toml`
3. first available configured bundle
4. empty browsing context when no bundle is available

Bundle resolution for session-selected `agentmux send` SHALL be:

1. explicit CLI `--bundle` when present
2. `default-bundle` from active `ui.toml`
3. fail-fast `validation_unknown_bundle`

Association-derived sender fallback SHALL NOT be used for these surfaces.

If selected session resolves to invalid sender identity, runtime SHALL fail with
`validation_unknown_sender`.
If selected session references unknown policy, runtime SHALL fail with
`validation_unknown_policy`.

#### Scenario: Resolve sender and bundle from explicit selectors

- **WHEN** invocation includes `--bundle agentmux --as-session user`
- **THEN** runtime resolves bundle `agentmux` and sender from session `user`

#### Scenario: Resolve sender and bundle from global defaults

- **WHEN** invocation omits selectors
- **AND** `ui.toml` provides `default-bundle` and `users.toml` provides
  `default-session`
- **THEN** runtime resolves bundle/session from those defaults

#### Scenario: Fall back to an available bundle when tui default is missing

- **WHEN** `agentmux tui` omits `--bundle`
- **AND** `default-bundle` is absent in `ui.toml`
- **THEN** runtime resolves the browsing bundle from the first available
  configured bundle, or an empty browsing context when none is available

#### Scenario: Reject send when default bundle is missing

- **WHEN** `agentmux send` omits `--bundle`
- **AND** `default-bundle` is absent in `ui.toml`
- **THEN** runtime returns `validation_unknown_bundle`

### Requirement: TUI Sender Configuration Files

The runtime SHALL support global user session configuration at relative path
`users.toml`, resolved through the shared effective-file lookup across the
configuration layers so a copy in an earlier layer shadows a copy in a later
one. Resolution SHALL NOT depend on build profile.

Supported fields SHALL use kebab-case and include:

- `default-session` (optional)
- `[[sessions]]` entries with:
  - required `id` (in `session@GLOBAL` canonical form)
  - required exactly one coder-less marker subtable: `[sessions.ui]` (TUI
    operators) or `[sessions.pubsub]` (embedded agents)
  - optional `name`
  - optional `policy`

`users.toml` is the identity and policy file; UI-surface operational defaults
such as `default-bundle` live in `ui.toml` (see `ui-surface-configuration`).

Global user sessions are coder-less by construction; a `coder` reference is
not accepted in `users.toml` entries.

Missing files SHALL not be treated as errors.
Malformed files SHALL fail fast with structured bootstrap validation errors.
Session `id` SHALL be in `session@GLOBAL` canonical form and SHALL be unique
within the file.

#### Scenario: Resolve sender from session entry in global users.toml

- **WHEN** runtime selects session `user@GLOBAL`
- **AND** `[[sessions]]` in `users.toml` contains `id = "user@GLOBAL"`
- **THEN** runtime resolves sender identity as `user@GLOBAL`

#### Scenario: Earlier layer users.toml shadows a later one in every build

- **WHEN** `users.toml` exists under two configuration layers
- **THEN** the copy from the earlier layer is used
- **AND** the result is identical in debug and release builds

#### Scenario: Reject unknown configured default session

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `users.toml`
- **THEN** startup fails with stable validation code

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

### Requirement: Session Type Validation in Config Load

The runtime SHALL validate session shape exclusivity at config load time:

- A bundle `[[sessions]]` entry SHALL declare exactly one shape: a coder-backed
  shape (a flat `coder` reference, with optional `coder-session-id`) or a
  coder-less shape (exactly one `[sessions.ui]` or `[sessions.pubsub]` marker
  subtable).
- A global `users.toml` entry SHALL declare exactly one coder-less marker
  subtable; a `coder` reference is not accepted.
- Zero shapes, or more than one shape, SHALL fail fast with a structured
  config error.
- A `coder-session-id` on a coder-less session SHALL fail fast with a
  structured config error.
- Unrecognized subtable keys SHALL fail fast with a structured config error.

A coder-less `[sessions.ui]` or `[sessions.pubsub]` marker with an empty body
SHALL be valid at parse time. Runtime MAY emit
`runtime_session_type_not_implemented` at startup for these types without
treating the configuration itself as invalid.

#### Scenario: Reject session entry with neither coder nor marker

- **WHEN** a `[[sessions]]` entry declares no `coder` reference and no
  coder-less marker subtable
- **THEN** config load fails with a structured validation error

#### Scenario: Reject session entry declaring both coder and marker

- **WHEN** a `[[sessions]]` entry declares a `coder` reference and also a
  `[sessions.ui]` marker subtable
- **THEN** config load fails with a structured validation error

#### Scenario: Reject coder-session-id on coder-less session

- **WHEN** a `[[sessions]]` entry declares a `[sessions.ui]` marker and a
  `coder-session-id`
- **THEN** config load fails with a structured validation error

#### Scenario: Accept ui session with empty marker body

- **WHEN** a `[[sessions]]` entry declares `[sessions.ui]` with no additional
  fields
- **THEN** config load succeeds

### Requirement: Relay Configuration File

The runtime SHALL support a relay-level configuration artifact at
`<config-root>/relay.toml`. The file itself is the relay configuration table;
relay-wide keys SHALL NOT be nested under an additional `[relay]` table. The
file SHALL use kebab-case TOML keys and MAY contain:

- `watch-bundles` (boolean, default `true`)
- `require-session-credentials` (boolean, default `false`)
- `[choices].pending-max`
- `[delivery]` table governing the relay's delivery scheduling, admission, and
  queue observability:

  | Key | Default | Range | Governs |
  |---|---|---|---|
  | `submission-timeout-ms` | `5_000` | `500..=60_000` | how long an authorized batch's ingestion may run before the relay initiates the generation fence |
  | `fence-observation-timeout-ms` | `5_000` | `100..=60_000` | the budget for each of the generation fence's two cessation observations, so total acknowledgment is bounded by twice this value |
  | `queued-envelopes-max` | `10_000` | `1..=1_000_000` | relay-global admission quota, envelope count |
  | `queued-bytes-max` | `268_435_456` | `1_048_576..=4_294_967_296` | relay-global admission quota, canonical payload bytes |
  | `queued-envelopes-per-target-max` | `1_000` | `1..=1_000_000` | per-target admission quota, envelope count |
  | `queued-bytes-per-target-max` | `33_554_432` | `1_048_576..=4_294_967_296` | per-target admission quota, canonical payload bytes |
  | `undelivered-warning-ms` | `1_800_000` | `60_000..=86_400_000` | how long a target's oldest `Pending` entry may age before the relay emits that target's first-crossing warning inscription |
  | `undelivered-report-interval-ms` | `300_000` | `30_000..=3_600_000` | cadence of the periodic undelivered-queue aggregate inscription |

- top-level `[[peers]]` entries with required `alias`, `address`, and
  `connect-as` string fields (see Outbound Peer Relay Configuration and Relay
  Cross-Relay Presented Identity)

The `[delivery]` keys live here rather than in `coders.toml` because they
describe the relay's own queue, scheduling, and reporting rather than any coder's
behavior.

**No `[delivery]` key bounds how long the relay waits for a *reachable* target to
become ready, and no configuration SHALL introduce one.** Such an entry waits
without a duration bound, subject to one deliberate exception — a fail-stopped
worker resolves every member it holds rather than stranding it behind a
generation that can never be replaced; see the `delivery-quiescence` capability's
`Async Queue Lifecycle and Ordering` requirement. `unreachable-dwell-ms` is not
an exception to this: it bounds how long a target may be continuously
*unreachable*, which qualifies a repeated observation rather than substituting
for an absent one.

`submission-timeout-ms` is the sole post-authorization bound, and it SHALL NOT be
read as a readiness bound. **It bounds ingestion, not readiness.** A batch is
authorized only once the relay has observed the target ready, and no transport
may wait on readiness afterwards, so the clock never covers a readiness wait.
What it covers is the transport consuming the bytes — in practice a single write
into a pty master, a child's stdin, or a subscriber channel.

Because readiness is advisory and can go stale between check and authorization,
**stale readiness is precisely how ingestion stalls**: the relay believed the
target was draining, began pushing bytes, and the target stopped. This is why the
default is small. Ingestion into a genuinely draining target completes in
microseconds; seconds of blocked ingestion mean the reader is not draining, not
that the write is large.

**Zero is not a permitted value for any `[delivery]` key, and no value denotes
"unlimited."** Every range above excludes zero, and a zero SHALL be rejected with
the same structured range error as any other out-of-range value. A zero quota
would reject every message and a zero fence observation budget would declare a
negative fence before any executor could be observed, so overloading zero as "no
limit" would make the two most dangerous misconfigurations indistinguishable from
the safest intent.

**Per-target quota SHALL NOT exceed relay-global quota in either dimension.**
`queued-envelopes-per-target-max` greater than `queued-envelopes-max`, or
`queued-bytes-per-target-max` greater than `queued-bytes-max`, SHALL fail
validation at load with a structured error naming both keys and both values. A
per-target limit above the global one is unreachable and therefore always a
configuration mistake.

**The undelivered-queue keys govern reporting only.** `undelivered-warning-ms`
and `undelivered-report-interval-ms` SHALL NOT influence any member's outcome,
release any admission quota, or alter scheduling. Their sole effect on elapse is
the emission of an inscription; see the `delivery-quiescence` capability's `Async
Delivery Observability` requirement for the emission rules.

`undelivered-warning-ms` SHALL default above the longest plausible agent turn, so
that a target legitimately mid-turn does not routinely produce warnings, and its
upper bound SHALL be permissive enough that an operator running long-horizon
agents can quiet it. Because zero is not permitted, the setting cannot be switched
off; raising it is the supported way to reduce its volume. It has no lower bound
tied to a turn length, because a short threshold produces noise rather than
incorrect behavior.

Missing `relay.toml` SHALL use the documented defaults. Malformed `relay.toml`,
unknown fields, wrong field types, and invalid peer entries SHALL fail startup
and pre-flight configuration validation with structured validation errors.

#### Scenario: Defaults when relay.toml is absent

- **WHEN** the configuration root has no `relay.toml`
- **THEN** relay startup uses `watch-bundles = true`
- **AND** uses `require-session-credentials = false`
- **AND** uses the documented `[delivery]` defaults
- **AND** has no configured outbound peers

#### Scenario: Load explicit relay controls

- **WHEN** `relay.toml` contains `watch-bundles = false`
- **AND** `require-session-credentials = true`
- **THEN** relay startup uses those relay-level settings

#### Scenario: Load explicit undelivered reporting settings

- **WHEN** `relay.toml` contains a `[delivery]` table setting
  `undelivered-warning-ms` and `undelivered-report-interval-ms` within their
  permitted ranges
- **THEN** relay startup uses those values for undelivered-queue reporting
- **AND** no member's outcome, quota, or scheduling position depends on either
  value

#### Scenario: Bound authorized execution

- **WHEN** `relay.toml` sets `[delivery].submission-timeout-ms` within the
  permitted range
- **THEN** the relay initiates that batch's generation fence and terminalizes no
  member at the bound
- **AND** every still-unresolved member is terminalized through the guard's
  evidence order at the fence verdict, not at the bound
- **AND** the setting is documented as an execution watchdog over the relay's own
  code, not as a judgement about target health

#### Scenario: Reject an out-of-range undelivered warning threshold

- **WHEN** `[delivery].undelivered-warning-ms` is below `60_000` or above
  `86_400_000`
- **THEN** relay startup fails with a structured error naming the key and the
  permitted range
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject zero for a delivery setting

- **WHEN** any `[delivery]` key is set to `0`
- **THEN** relay startup fails with the structured range error for that key
- **AND** the zero is not interpreted as "unlimited"

#### Scenario: Reject per-target quota above the global quota

- **WHEN** `[delivery].queued-envelopes-per-target-max` exceeds
  `queued-envelopes-max`, or `queued-bytes-per-target-max` exceeds
  `queued-bytes-max`
- **THEN** relay startup fails with a structured validation error naming both
  keys and both values

#### Scenario: Reject nested relay table

- **WHEN** `relay.toml` contains a `[relay]` table
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject malformed relay TOML syntax

- **WHEN** `relay.toml` is not syntactically valid TOML
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject unknown relay configuration field

- **WHEN** `relay.toml` contains an unknown top-level field
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject wrong relay configuration field type

- **WHEN** `relay.toml` contains `watch-bundles = 'false'`
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

### Requirement: Relay Configuration Precedence

Relay runtime settings SHALL resolve with this precedence, highest to lowest:
CLI override, environment override, `relay.toml`, documented defaults. CLI
overrides SHALL include `agentmux host relay --no-watch` for
`watch-bundles = false` and `agentmux host relay --require-credentials` for
`require-session-credentials = true`. Environment overrides SHALL include
`AGENTMUX_RELAY_WATCH_BUNDLES` and
`AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS`, parsed as canonical boolean
strings: exactly `true` or `false`. Invalid environment override values SHALL
fail startup with structured validation errors.

This precedence ladder applies to `watch-bundles` and
`require-session-credentials`. `[choices].pending-max` and `[[peers]]` SHALL
resolve from `relay.toml` or documented defaults only; this proposal does not
define CLI or environment overrides for those settings.

#### Scenario: CLI override wins over relay.toml

- **WHEN** `relay.toml` contains `watch-bundles = true`
- **AND** the operator runs `agentmux host relay --no-watch`
- **THEN** relay startup resolves `watch-bundles = false`

#### Scenario: Environment override wins over relay.toml

- **WHEN** `relay.toml` contains `require-session-credentials = false`
- **AND** `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS=true` is set
- **THEN** relay startup resolves `require-session-credentials = true`

#### Scenario: Accept canonical boolean environment override values

- **WHEN** `AGENTMUX_RELAY_WATCH_BUNDLES=false` is set
- **AND** `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS=true` is set
- **THEN** relay startup accepts both environment override values

#### Scenario: relay.toml wins over defaults

- **WHEN** `relay.toml` contains `watch-bundles = false`
- **AND** no CLI or environment override is supplied for watch behavior
- **THEN** relay startup resolves `watch-bundles = false`

#### Scenario: Reject invalid environment override

- **WHEN** `AGENTMUX_RELAY_WATCH_BUNDLES=maybe` is set
- **THEN** relay startup fails with a structured validation error

#### Scenario: No override for choices or peers

- **WHEN** `[choices].pending-max` is absent from `relay.toml`
- **AND** no `[[peers]]` entries exist in `relay.toml`
- **THEN** relay startup uses the documented choices default
- **AND** has no configured outbound peers

### Requirement: Outbound Peer Relay Configuration

Relay configuration SHALL support top-level `[[peers]]` entries that define
outbound peer relay routing. `[[peers]]` is purely an outbound routing table; it
carries no inbound authorization. Each peer entry SHALL carry:

- `alias`: a required non-empty string — this relay's **local** name for the
  peer. It serves as the peer's `<alias>` in cross-relay bang-path addressing
  (`<session>@<bundle>!<alias>`) and as the `<alias>` in the credential file path.
  It is internal to this relay and never presented to the peer. Grammar: a bare
  relay id (non-empty; no `@`, `!`, or path separator).
- `address`: a required outbound endpoint. In this slice `address` SHALL be an
  **absolute filesystem path** to a Unix domain socket (same-host peers), the
  transport the relay presently serves. A non-absolute value, or a `host:port`
  TCP-style endpoint, SHALL be rejected at startup and pre-flight validation with
  a structured error — the fail-fast counterpart of the remote/TCP non-goal,
  rather than deferring the failure to an unreachable-socket delivery outcome. A
  `host:port` TCP endpoint is the documented future shape once the relay gains a
  TCP listener and is not yet a supported target.
- `connect-as`: a required non-empty bare relay id — the identity this relay
  presents to the peer (`<connect-as>@RELAY`), determined by the peer (see Relay
  Cross-Relay Presented Identity).

Inbound authorization for a peer relay — what an inbound request carried by that
peer may reach on this relay — is NOT configured here. It is the `scope` recorded
on the peer relay principal's store record when its credential is registered via
`new peer <id>@RELAY`, and is read by the ingress filter (see the
`relay-routing-layer` capability). A relay that only receives from a peer
therefore needs no `[[peers]]` entry for it — only a registered credential.

Unknown peer entry fields SHALL fail startup and pre-flight configuration
validation with structured validation errors. Peer entries SHALL NOT contain raw
PSK material; raw peer relay PSKs SHALL remain owner-only state artifacts at
`<state-root>/peers/<alias>.psk` (mode 0600), while the principal store records
credential hashes.

The relay SHALL NOT open an outbound peer connection at startup solely because a
peer entry exists; connections are established lazily on first cross-relay
delivery to that peer (see the `cross-relay-routing` capability). A peer whose
endpoint is unreachable at startup SHALL NOT block or fail relay startup.

#### Scenario: Validate outbound peer entry

- **WHEN** `relay.toml` contains a `[[peers]]` entry with a non-empty `alias`, an
  absolute `address` Unix socket path, and a non-empty bare-id `connect-as`
- **THEN** configuration validation accepts the entry
- **AND** relay startup does not attempt an outbound peer connection

#### Scenario: Reject non-absolute or TCP-style peer address

- **WHEN** a `[[peers]]` entry's `address` is not an absolute path — e.g. a
  `host:port` TCP endpoint or a relative path
- **THEN** relay startup fails with a structured validation error naming the
  `peers.address` field
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject peer entry missing alias or connect-as

- **WHEN** a `[[peers]]` entry omits (or leaves empty) `alias` or `connect-as`
- **THEN** relay startup fails with a structured validation error naming the
  offending field
- **AND** `agentmux check configuration` reports the same invalid artifact

#### Scenario: Reject malformed peer entry

- **WHEN** a `[[peers]]` entry omits `address` or carries an unknown field
- **THEN** relay startup fails with a structured validation error
- **AND** `agentmux check configuration` reports the same invalid artifact

### Requirement: Relay Cross-Relay Presented Identity

The identity this relay presents to a peer SHALL be configured **per peer**, not
relay-wide: the *receiving* relay determines the identity it expects (via its own
`new peer`), and two peers MAY issue this relay different — or colliding —
identities, so no single relay-wide identity exists. Each `[[peers]]` entry SHALL
carry a `connect-as` string naming the bare relay id that peer issued this relay;
the relay composes `<connect-as>@RELAY` and presents it as its own principal in
the outbound Hello it sends to that peer. A relay that only receives from a peer
needs no `[[peers]]` entry and presents no identity to it.

`connect-as` SHALL be a **bare relay id**: non-empty after trimming surrounding
whitespace, carrying no namespace suffix (`@`), no cross-relay delimiter (`!`),
and no path separators — the relay composes the `@RELAY` suffix itself, so an
already-qualified value such as `east@RELAY` is invalid rather than becoming
`east@RELAY@RELAY`. A `[[peers]]` entry that omits `connect-as`, or supplies one
that is empty/whitespace or not a bare relay id, SHALL fail startup and pre-flight
configuration validation with a structured validation error.

#### Scenario: Reject qualified or malformed connect-as

- **WHEN** a `[[peers]]` entry sets `connect-as` to a value that is not a bare
  relay id — e.g. one carrying an `@` suffix (`east@RELAY`), a `!` delimiter, a
  path separator, or only whitespace
- **THEN** relay startup fails with a structured validation error naming the
  `peers.connect-as` field
- **AND** `agentmux check configuration` reports the same invalid artifact

### Requirement: Bring-Up Association Environment Injection

Configuration load SHALL stamp authoritative bring-up context into each
coder-backed member's merged spawn environment, so a launched agent propagates it
to its `agentmux host mcp` subprocess and association resolution consults it
rather than inferring identity from the filesystem.

The stamped context SHALL include the hosting bundle name as `AGENTMUX_BUNDLE`
and the member id as `AGENTMUX_SESSION`, and SHALL be extensible to further
context without redefining the mechanism.

Bundle and session context SHALL be stamped upsert-if-absent: an
operator-declared environment entry of the same name SHALL be left untouched.

The relay's normalized state root SHALL additionally be injected as
`AGENTMUX_STATE_DIRECTORY` at spawn time, authoritatively, overwriting any value
already present from coder, bundle, or member configuration. This differs from
bundle and session context deliberately, on two grounds.

First, the value is not known at configuration load. The state root belongs to
the relay performing the spawn, not to the configuration being loaded, so
load-time injection would have to invent or re-derive it.

Second, upsert-if-absent cannot express this contract. A child exists to reach
the relay that spawned it; an operator-declared or blank `AGENTMUX_STATE_DIRECTORY`
would suppress the stamp and send the child to a different relay, which is not an
override of a preference but a broken rendezvous. There is no legitimate reason
for a member of one relay to address another — cross-relay communication is
expressed by configured peers, not by children attaching elsewhere.

The value injected SHALL be the normalized absolute state root, so it does not
re-resolve against the child's working directory.

Spawned coder processes receive the context directly; `agentmux host mcp` is a
descendant of the coder rather than a child of the relay, and receives the
context by ordinary environment inheritance.

Generated coder client configuration SHALL NOT emit `--state-directory`. A
template-generated command line is committed content, so a flag in it would
outrank the environment value and silently defeat the rendezvous the injection
exists to guarantee.

- The context SHALL be stamped only for coder-backed members; coder-less members
  (`ui`/`pubsub`) spawn no agent and SHALL carry no injected context.
- A blank value SHALL be treated as absent by every consumer, for both
  resolution and any classification derived from presence.

#### Scenario: Stamp context onto a coder member

- **WHEN** a bundle configuration is loaded
- **AND** a coder-backed member declares no `AGENTMUX_BUNDLE`/`AGENTMUX_SESSION`
  environment entries
- **THEN** the member's spawn environment includes `AGENTMUX_BUNDLE` set to the
  hosting bundle name and `AGENTMUX_SESSION` set to the member id

#### Scenario: Preserve operator-declared context

- **WHEN** a coder-backed member explicitly declares an `AGENTMUX_BUNDLE`
  environment entry
- **THEN** configuration load leaves that entry's value untouched

#### Scenario: Skip injection for coder-less members

- **WHEN** a coder-less (`ui` or `pubsub`) member is loaded
- **THEN** its spawn environment carries no injected context entry

#### Scenario: Blank context value is absent at ingress

- **WHEN** a context variable is present in the process environment with a blank
  value
- **THEN** it is normalized to absent where the environment is read
- **AND** every consumer observes it identically as absent

#### Scenario: Inject the state root authoritatively at spawn

- **WHEN** a relay spawns a coder-backed member
- **THEN** the spawn environment carries `AGENTMUX_STATE_DIRECTORY` set to the
  relay's normalized absolute state root

#### Scenario: A configured state directory does not suppress the rendezvous

- **WHEN** a coder, bundle, or member declares `AGENTMUX_STATE_DIRECTORY`,
  whether with a conflicting value or a blank one
- **THEN** the relay's value is injected in its place

#### Scenario: A child stays on the relay that spawned it

- **WHEN** a relay is started with an explicit `--state-directory`
- **AND** it spawns a coder-backed member whose process runs with a working
  directory different from the relay's
- **THEN** the member's `agentmux host mcp` descendant resolves the spawning
  relay's state root
- **AND** reaches that relay's socket rather than the default root's

### Requirement: MCP Startup Fault Tolerance

`agentmux host mcp` SHALL fail at process start only when it cannot serve the
MCP protocol. Faults arising while constructing the operational context —
argument interpretation, root resolution, configuration loading, association
resolution, and runtime security posture — SHALL be retained and reported at
tool-invocation time.

Relay reachability SHALL NOT be a retained startup fault. It is evaluated per
request at tool time and surfaces as `relay_unavailable`, and a server whose
operational context is complete SHALL be `Ready` regardless of whether the relay
is currently connectable.

The server SHALL hold an explicit readiness state of either a ready context or a
retained startup fault.

- Process-time failure SHALL remain for faults arising before `host mcp` is
  identifiable as the requested command, for async runtime, router, stdio, or
  protocol serving failures, and for `--help`.
- The server SHALL NOT proceed on partially parsed arguments, and SHALL NOT fall
  through from malformed higher-level intent to a lower tier.
- Protocol initialization, tool listing, tool schemas, and `help` SHALL succeed
  regardless of readiness state.
- Each tool request SHALL be validated on its own terms before the readiness
  guard is consulted, so a malformed request reports its own fault.
- The retained fault SHALL be a snapshot and SHALL NOT be re-evaluated until the
  process restarts.

#### Scenario: Start green when the bundle is unknown

- **WHEN** bundle association resolves to a bundle with no configured definition
- **THEN** MCP startup succeeds
- **AND** the fault is retained

#### Scenario: Start green when configuration is malformed

- **WHEN** a required configuration file cannot be parsed
- **THEN** MCP startup succeeds
- **AND** the fault is retained

#### Scenario: Start green when startup arguments are invalid

- **WHEN** `host mcp` is identifiable but its arguments are invalid
- **THEN** MCP startup succeeds
- **AND** the fault is retained
- **AND** no partially parsed argument value is used

#### Scenario: Advertise tools regardless of readiness

- **WHEN** the server holds a retained startup fault
- **THEN** protocol initialization, tool listing, and tool schemas succeed
- **AND** the advertised tool inventory is unchanged

#### Scenario: Report the retained cause on tool invocation

- **WHEN** the server holds a retained startup fault
- **AND** a well-formed request is received for a tool that requires a resolved
  association, a loaded configuration, or relay access
- **THEN** the response is a structured error carrying the retained cause

#### Scenario: Tools needing no operational context still succeed

- **WHEN** the server holds a retained startup fault
- **AND** a well-formed request is received for a tool requiring none of those
- **THEN** the tool succeeds

#### Scenario: Malformed request reports its own fault

- **WHEN** the server holds a retained startup fault
- **AND** a tool request fails its own validation
- **THEN** the response reports the request's fault rather than the retained one

#### Scenario: Fail process start when the protocol cannot be served

- **WHEN** stdio transport or protocol router initialization fails
- **THEN** MCP process startup fails

### Requirement: Configuration Layer Resolution

The system SHALL resolve every configuration file through a single
effective-file lookup that consults each configuration layer in list order and
selects the first existing regular file. All relay, TUI, CLI, and preflight
loaders SHALL use this lookup.

- A malformed file in one layer SHALL be a fault and SHALL NOT fall through to a
  later layer.
- Directories of bundle definitions SHALL union by bundle identifier, with an
  entry in an earlier layer shadowing an entry of the same identifier in a later
  layer.
- Relative path-valued fields SHALL retain their existing per-field resolution
  base. A field SHALL resolve identically regardless of which layer supplied the
  file containing it; no layer SHALL become a resolution base and no layer SHALL
  alter any field's existing base.
- Starter configuration hydration SHALL occur only when the layer list was
  resolved from the XDG/home default tier, which is a single layer. A list
  supplied by CLI or environment SHALL never be scaffolded.

#### Scenario: Earlier layer shadows later layer

- **WHEN** the same relative path exists under two configuration layers
- **THEN** the file from the earlier layer is used

#### Scenario: Fall through to a later layer

- **WHEN** a relative path exists only under a later configuration layer
- **THEN** that file is used

#### Scenario: Malformed file does not fall through

- **WHEN** a file exists in one layer but cannot be parsed
- **THEN** the fault is reported
- **AND** the corresponding file in a later layer is not used

#### Scenario: Supplied layers are never scaffolded

- **WHEN** the layer list is supplied by CLI or environment
- **AND** a layer lacks starter configuration files
- **THEN** no starter configuration is written

#### Scenario: Missing supplied layer surfaces per command class

- **WHEN** a supplied configuration layer does not exist
- **THEN** `host mcp` retains the fault and reports it at tool-invocation time
- **AND** other commands report it immediately

#### Scenario: Bundle definitions union by identifier across layers

- **WHEN** the last layer defines bundles `alpha` and `beta`
- **AND** an earlier layer defines bundle `beta`
- **THEN** the effective set is `alpha` from the last layer and `beta` from the
  earlier one

#### Scenario: Relative paths do not rebase per layer

- **WHEN** a bundle file in one layer declares a relative member directory
- **THEN** it resolves against the same base as the identical declaration in a
  bundle file supplied by any other layer

#### Scenario: Watcher reconciles against the layer union

- **WHEN** a bundle definition is created in an earlier layer shadowing one in a
  later layer
- **THEN** the effective bundle reloads from the earlier layer
- **AND** removing it again reloads from the later layer rather than unloading

