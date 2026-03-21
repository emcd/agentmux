## MODIFIED Requirements

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

## REMOVED Requirements

### Requirement: Relay Group Startup Outcome Semantics

**Reason**: Group selectors are removed from `agentmux host relay` in this
change. Group-based lifecycle transitions move to `agentmux up/down`, which use
their own canonical transition payload contract.

## ADDED Requirements

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
