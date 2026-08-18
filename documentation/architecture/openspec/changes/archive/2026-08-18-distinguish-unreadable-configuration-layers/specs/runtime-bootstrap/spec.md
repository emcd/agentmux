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
