## MODIFIED Requirements

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
