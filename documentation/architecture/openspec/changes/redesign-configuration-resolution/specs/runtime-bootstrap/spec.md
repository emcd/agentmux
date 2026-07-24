## MODIFIED Requirements

### Requirement: XDG Configuration Root

The system SHALL resolve the configuration root using precedence:

1. explicit CLI `--configuration-directory` when present
2. `AGENTMUX_CONFIGURATION_DIRECTORY` environment variable when set and
   non-blank, resolved against the working directory when relative, identically
   to the CLI flag
3. nearest-ancestor discovery when discovery is enabled
4. `$XDG_CONFIG_HOME/agentmux` when set and non-empty, otherwise
   `~/.config/agentmux`

Tiers 1 and 2 SHALL **replace** the configuration root rather than extend a
search list, so an explicitly supplied root never falls through to a different
root for files it does not define.

Configuration root resolution SHALL NOT depend on build profile.

#### Scenario: Resolve configuration root from explicit CLI value

- **WHEN** startup receives `--configuration-directory`
- **THEN** the configuration root is that path
- **AND** discovery and XDG/home resolution are bypassed

#### Scenario: Resolve configuration root from environment

- **WHEN** no `--configuration-directory` is provided
- **AND** `AGENTMUX_CONFIGURATION_DIRECTORY` is set and non-blank
- **THEN** the configuration root is that path

#### Scenario: Explicit root does not fall through for undefined files

- **WHEN** the configuration root is supplied explicitly
- **AND** a requested configuration file does not exist under that root
- **THEN** resolution reports the file as absent
- **AND** no other configuration root is consulted

#### Scenario: Resolve configuration root from XDG default

- **WHEN** no explicit root is provided
- **AND** discovery is disabled or finds no marker
- **THEN** the configuration root resolves from `$XDG_CONFIG_HOME/agentmux` or
  `~/.config/agentmux`

#### Scenario: Configuration root resolution is identical across build profiles

- **WHEN** the same inputs are supplied to a debug build and a release build
- **THEN** both resolve the same configuration root

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

### Requirement: Sender Association Resolution

The MCP server SHALL resolve sender association at startup using precedence:

1. explicit CLI `--session-name` when present
2. injected bring-up environment variable `AGENTMUX_SESSION` when present and
   non-blank
3. overlay-resolved association file `session_name` when present
4. working-directory match against configured member directories

A blank injected value SHALL be treated as absent.

A tier SHALL apply only when every tier above it is absent. When a tier supplies
a sender that names no configured member, sender association SHALL be recorded as
unresolved with that cause and SHALL NOT fall through to a lower tier.

Sender association SHALL NOT be derived from Git metadata. When no tier supplies
a sender and no configured member matches, sender association SHALL be recorded
as unresolved rather than failing startup.

The tier which supplied the resolved sender SHALL be recorded.

#### Scenario: Resolve sender from explicit CLI value

- **WHEN** MCP startup has explicit `--session-name`
- **THEN** sender association is set to that configured session

#### Scenario: Injected environment wins over association file

- **WHEN** CLI sender is absent
- **AND** the `AGENTMUX_SESSION` environment value is present and non-blank
- **AND** the association file also provides `session_name`
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

- **WHEN** a sender is supplied by CLI, environment, or association file
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

### Requirement: Bundle Association Resolution

The MCP server SHALL resolve bundle association at startup using precedence:

1. explicit CLI `--bundle` when present
2. injected bring-up environment variable `AGENTMUX_BUNDLE` when present and
   non-blank
3. overlay-resolved association file `bundle_name` when present
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

The tier which supplied the resolved bundle SHALL be recorded.

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
- **AND** the association file provides `bundle_name`
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
effective-file lookup. The lookup selects `<root>/overlay/mcp.toml` when present
and otherwise `<root>/mcp.toml`; the overlay segment SHALL NOT appear in the
logical path, so it is applied exactly once.

The resolved artifact is the **effective association file**, and it occupies a
single tier in each association ladder.

Supported override fields SHALL be:

- `bundle_name`
- `session_name`

Fields SHALL be independently optional: a file supplying only one field SHALL
leave the other to the remaining association tiers.

The file SHALL NOT support a configuration-root field. A file located beneath the
configuration root cannot redirect the configuration root.

#### Scenario: Ignore missing association file

- **WHEN** neither `<root>/overlay/mcp.toml` nor `<root>/mcp.toml` exists
- **THEN** startup continues using the remaining association tiers

#### Scenario: Overlay association file shadows the base

- **WHEN** both `<root>/overlay/mcp.toml` and `<root>/mcp.toml` exist
- **THEN** the overlay file is the effective association file
- **AND** the base file contributes no fields

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

### Requirement: Override Directory VCS Posture

The project SHALL commit its Agentmux configuration directory and SHALL
Git-ignore the overlay directory beneath it, so shared configuration is tracked
while per-working-tree divergence is not.

#### Scenario: Track configuration directory in Git

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/agentmux/` is tracked

#### Scenario: Ignore overlay directory in Git

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/agentmux/overlay/` is ignored

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

### Requirement: TUI Sender Configuration Files

The runtime SHALL support global user session configuration at relative path
`users.toml`, resolved through the shared effective-file lookup so an
overlay-provided file shadows the base file. Resolution SHALL NOT depend on
build profile.

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

#### Scenario: Overlay users.toml shadows the base file in every build

- **WHEN** `users.toml` exists under both the overlay and the base root
- **THEN** the overlay file is used
- **AND** the result is identical in debug and release builds

#### Scenario: Reject unknown configured default session

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `users.toml`
- **THEN** startup fails with stable validation code

### Requirement: TUI Override File VCS Posture

The global users overlay file SHALL follow the overlay VCS posture so per-user
test defaults do not leak into shared tracked configuration.

#### Scenario: Keep overlay users.toml under the ignored overlay directory

- **WHEN** repository ignore rules are evaluated
- **THEN** `.auxiliary/configuration/agentmux/overlay/users.toml` is covered by
  the ignored overlay path

## ADDED Requirements

### Requirement: Bring-Up Association Environment Injection

Configuration load SHALL stamp authoritative bring-up context into each
coder-backed member's merged spawn environment, so a launched agent propagates it
to its `agentmux host mcp` subprocess and association resolution consults it
rather than inferring identity from the filesystem.

The stamped context SHALL include the hosting bundle name as `AGENTMUX_BUNDLE`
and the member id as `AGENTMUX_SESSION`, and SHALL be extensible to further
context without redefining the mechanism.

- The context SHALL be stamped only for coder-backed members; coder-less members
  (`ui`/`pubsub`) spawn no agent and SHALL carry no injected context.
- The stamp SHALL be upsert-if-absent: an operator-declared environment entry of
  the same name SHALL be left untouched.
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

### Requirement: Configuration Overlay Resolution

The system SHALL resolve every configuration file through a single effective-file
lookup that consults, in order, `<root>/overlay/<path>` then `<root>/<path>`, and
selects the first existing regular file. All relay, TUI, CLI, and preflight
loaders SHALL use this lookup.

- A malformed overlay file SHALL be a fault and SHALL NOT fall through to the
  base file.
- Directories of bundle definitions SHALL union by bundle identifier, with an
  overlay entry shadowing a base entry of the same identifier.
- Relative path-valued fields SHALL retain their existing per-field resolution
  base. A field supplied by an overlay file SHALL resolve identically to the
  same field supplied by the corresponding base file; the overlay directory
  SHALL NOT become a resolution base and SHALL NOT alter any field's existing
  base.
- Starter configuration hydration SHALL occur only when the configuration root
  was resolved from the XDG/home default tier. A root supplied by CLI,
  environment, or discovery SHALL never be scaffolded, and SHALL never have an
  overlay directory created for it.

#### Scenario: Overlay file shadows base file

- **WHEN** the same relative path exists under both the overlay and the base root
- **THEN** the overlay file is used

#### Scenario: Fall through to base when overlay lacks the file

- **WHEN** a relative path exists only under the base root
- **THEN** the base file is used

#### Scenario: Malformed overlay file does not fall through

- **WHEN** an overlay file exists but cannot be parsed
- **THEN** the fault is reported
- **AND** the corresponding base file is not used

#### Scenario: Explicit root is never scaffolded

- **WHEN** the configuration root is supplied by CLI, environment, or discovery
- **AND** it lacks starter configuration files
- **THEN** no starter configuration is written

#### Scenario: Missing explicit root surfaces per command class

- **WHEN** the configuration root is supplied explicitly and does not exist
- **THEN** `host mcp` retains the fault and reports it at tool-invocation time
- **AND** other commands report it immediately

#### Scenario: Bundle definitions union by identifier

- **WHEN** the base root defines bundles `alpha` and `beta`
- **AND** the overlay defines bundle `beta`
- **THEN** the effective set is `alpha` from the base and `beta` from the overlay

#### Scenario: Relative paths do not rebase under the overlay

- **WHEN** an overlay bundle file declares a relative member directory
- **THEN** it resolves against the same base as the identical declaration in a
  base bundle file
- **AND** that field's existing resolution base is unchanged by this requirement

### Requirement: Configuration Root Discovery

The system SHALL support opt-in discovery of a configuration root, enabled by
the `--discover-local-configuration` flag and disabled by default.

Discovery SHALL enumerate the working directory and each of its ancestors. For
each candidate ancestor `A`, the candidate configuration root SHALL be
`A/.auxiliary/configuration/agentmux`. A candidate SHALL be valid when that path
exists and is a directory. The candidate derived from the nearest ancestor SHALL
win.

- Enumeration SHALL begin at the canonicalized working directory and terminate
  at the filesystem root.
- Paths SHALL be canonicalized before enumeration so symbolic links resolve
  consistently, and the selected root SHALL be reported in canonical form.
- Discovery SHALL NOT depend on build profile, Git metadata, or package
  manifests.
- The selected root SHALL be reported on a diagnostic channel that is never the
  MCP stdio stream, so a diagnostic cannot corrupt the protocol.

#### Scenario: Discovery disabled by default

- **WHEN** `--discover-local-configuration` is not supplied
- **AND** an ancestor of the working directory contains
  `.auxiliary/configuration/agentmux`
- **THEN** it is not used
- **AND** resolution falls through to the XDG/home default

#### Scenario: Discover root from an ancestor of the working directory

- **WHEN** discovery is enabled
- **AND** the working directory is `/repo/subdir`
- **AND** `/repo/.auxiliary/configuration/agentmux` exists and is a directory
- **THEN** the configuration root is `/repo/.auxiliary/configuration/agentmux`

#### Scenario: Nearest ancestor wins

- **WHEN** discovery is enabled
- **AND** both `/repo/.auxiliary/configuration/agentmux` and
  `/repo/nested/.auxiliary/configuration/agentmux` exist
- **AND** the working directory is under `/repo/nested`
- **THEN** the configuration root derived from `/repo/nested` is selected

#### Scenario: Discovery finds no candidate

- **WHEN** discovery is enabled
- **AND** no ancestor yields an existing candidate directory
- **THEN** resolution falls through to the XDG/home default

#### Scenario: Report the selected root off the protocol stream

- **WHEN** discovery selects a configuration root during `host mcp` startup
- **THEN** the selected root is reported on a diagnostic channel
- **AND** nothing is written to the MCP stdio stream

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
