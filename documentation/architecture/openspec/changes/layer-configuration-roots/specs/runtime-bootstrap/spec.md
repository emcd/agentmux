## MODIFIED Requirements

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

## ADDED Requirements

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

## REMOVED Requirements

### Requirement: Configuration Overlay Resolution

**Reason**: Replaced by Configuration Layer Resolution. The `overlay/`
subdirectory fixed the layer count at two and anchored the second layer to a
directory name, neither of which survives configuration moving out of the
project being worked on.

**Migration**: Name the former overlay directory as a layer ahead of the base.

### Requirement: Configuration Root Discovery

**Reason**: Discovery located a configuration root inside the project being
worked on. Configuration no longer lives inside projects, removing the case it
was built for, and an explicit layer list serves the same deployments by naming
the target rather than inferring it.

**Migration**: Supply the intended root with `--configuration-directory`.
