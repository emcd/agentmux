## MODIFIED Requirements

### Requirement: Configuration Pre-flight Command Surface

The system SHALL provide an `agentmux check configuration [<bundle-id>]`
subcommand that validates bundle configuration through the same loading path the
relay uses at startup, without starting the relay or mutating configuration.

The command SHALL accept an optional positional `<bundle-id>`: when present it
validates that single bundle; when omitted it validates every bundle in the
effective bundle set, which is the union of the overlay and base bundles
directories with overlay entries shadowing base entries of the same identifier.

The command SHALL inherit the global runtime flags
(`--configuration-directory`, `--state-directory`,
`--inscriptions-directory`/`--logs-directory`, `--repository-root`).

Validation SHALL cover bundle and coders schema and authorization-policy
resolution (`policies.toml`, `relay.toml`, and `users.toml` policy mappings),
matching what the relay rejects at startup, and SHALL resolve every file through
the shared effective-file lookup so it validates what the relay would actually
load.

The command SHALL be read-only: it MUST NOT scaffold or modify configuration
artifacts.

On success the command SHALL exit zero. On the first invalid bundle it SHALL
exit non-zero and report the offending file path and field-level detail; it does
not partially load or degrade gracefully. The reported path SHALL be the
physical file selected by the effective lookup, so an operator can tell whether
an overlay or a base file is at fault.

#### Scenario: Validate a single named bundle

- **WHEN** an operator runs `agentmux check configuration <bundle-id>` against a
  valid configuration
- **THEN** the command exits zero
- **AND** reports the bundle as validated

#### Scenario: Validate every bundle in the effective set

- **WHEN** an operator runs `agentmux check configuration` with no positional
  argument
- **THEN** validation covers the union of overlay and base bundle definitions
- **AND** an overlay definition shadows a base definition of the same identifier
- **AND** exits zero when all are valid

#### Scenario: Report an unknown configuration field

- **WHEN** a bundle file contains an unknown field (for example a misspelled
  session key)
- **THEN** the command exits non-zero
- **AND** reports the offending file path and the offending field

#### Scenario: Report the physical file at fault

- **WHEN** validation fails on a bundle supplied by the overlay
- **THEN** the reported path is the overlay file rather than the base file

#### Scenario: Reject an unknown check subcommand

- **WHEN** an operator runs `agentmux check <other>` where `<other>` is not
  `configuration`
- **THEN** the command rejects the invocation with a structured argument
  validation error

#### Scenario: Report when no bundles are discoverable

- **WHEN** an operator runs `agentmux check configuration` with no positional
  argument and no bundle files exist in either layer
- **THEN** the command exits non-zero
- **AND** reports that no bundle configurations were found

#### Scenario: Remain read-only

- **WHEN** the command runs against a configuration root missing starter files
- **THEN** no configuration artifact is created or modified

## ADDED Requirements

### Requirement: Configuration Root Command-Line Surface

The global runtime flag selecting the configuration root SHALL be named
`--configuration-directory`. It SHALL be honored identically in every build
profile.

The `--discover-local-configuration` flag SHALL enable ancestor-based
configuration-root discovery and SHALL default to disabled.

`--repository-root` SHALL no longer influence configuration-root resolution. It
SHALL retain its existing role in state and inscriptions root resolution until
the deferred runtime-instance work replaces it, so repository-local runtime data
remains reachable and a source-tree relay does not collide with an installed
one.

#### Scenario: Select configuration root in any build profile

- **WHEN** an operator passes `--configuration-directory <path>`
- **THEN** the configuration root is that path
- **AND** the behavior is identical in debug and release builds

#### Scenario: Accept a relative configuration directory

- **WHEN** an operator passes a relative `--configuration-directory`
- **THEN** it resolves against the current working directory

#### Scenario: Discovery is off unless requested

- **WHEN** `--discover-local-configuration` is not supplied
- **THEN** ancestor discovery does not run

#### Scenario: Repository root no longer selects the configuration root

- **WHEN** an operator passes `--repository-root`
- **THEN** the configuration root is unaffected

#### Scenario: Repository root still selects state and inscriptions roots

- **WHEN** an operator passes `--repository-root`
- **THEN** state and inscriptions root resolution continue to honor it

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
- **AND** no explicit, injected, or overlay bundle is present
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
