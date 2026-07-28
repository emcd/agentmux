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

`--repository-root` SHALL NOT influence configuration-root resolution. It SHALL
retain its existing role in state and inscriptions root resolution until the
deferred runtime-instance work replaces it, so repository-local runtime data
remains reachable and a source-tree relay does not collide with an installed
one.

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

#### Scenario: Repository root no longer selects configuration layers

- **WHEN** an operator passes `--repository-root`
- **THEN** the layer list is unaffected

#### Scenario: Repository root still selects state and inscriptions roots

- **WHEN** an operator passes `--repository-root`
- **THEN** state and inscriptions root resolution continue to honor it
