## ADDED Requirements
### Requirement: Bundle Group Resolution

Relay host group selection SHALL resolve membership from bundle-local
configuration under:

- `<config-root>/bundles/<bundle-id>.toml`

Bundle files MAY define optional top-level:

- `groups` (`string[]`)

Group naming rules:

- reserved/system group names are uppercase
- custom group names are lowercase
- MVP reserved group `ALL` is implicit and selects all configured bundles

#### Scenario: Resolve custom group from bundle-local groups

- **WHEN** an operator invokes `agentmux host relay --group dev`
- **THEN** the system selects bundles whose `groups` include `dev`

#### Scenario: Resolve ALL as implicit group

- **WHEN** an operator invokes `agentmux host relay --group ALL`
- **THEN** the system selects all configured bundles
- **AND** does not require explicit `ALL` membership in bundle files

#### Scenario: Treat missing groups key as no custom group membership

- **WHEN** a bundle file omits `groups`
- **THEN** that bundle is still selectable by `<bundle-id>` and `--group ALL`
- **AND** it is not selected for custom groups unless explicitly listed

#### Scenario: Reject unknown custom group

- **WHEN** an operator invokes `agentmux host relay --group nightly`
- **AND** no configured bundle contains group `nightly`
- **THEN** the system rejects invocation with `validation_unknown_group`

#### Scenario: Reject invalid custom uppercase group name

- **WHEN** an operator invokes `agentmux host relay --group DEV`
- **AND** `DEV` is not a reserved system group
- **THEN** the system rejects invocation with `validation_invalid_group_name`

### Requirement: Relay Group Startup Outcome Semantics

In `--group` mode, runtime startup SHALL use partial-host semantics across
selected bundles.

Per selected bundle, startup SHALL report one outcome:

- `hosted`
- `skipped`
- `failed`

Per selected bundle, startup SHALL also report:

- `reason_code` (nullable)
- `reason` (nullable human text)

Startup summary output in group mode SHALL include aggregate fields:

- `hosted_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `hosted_any` (boolean)

Process exit status in group mode SHALL be non-zero only when zero bundles are
successfully hosted.

#### Scenario: Continue hosting when one selected bundle lock is held

- **WHEN** at least one selected bundle is hostable
- **AND** one or more selected bundles are lock-held
- **THEN** the system hosts available bundles
- **AND** reports lock-held bundles as `skipped`
- **AND** sets `reason_code=lock_held` for those bundles

#### Scenario: Return non-zero when zero bundles host

- **WHEN** `agentmux host relay --group dev` completes startup with zero
  `hosted` bundles (`hosted_bundle_count == 0`)
- **THEN** process startup returns non-zero exit status

#### Scenario: Return success when at least one selected bundle hosts

- **WHEN** `agentmux host relay --group dev` completes startup with one or more
  `hosted` bundles
- **THEN** process startup returns success exit status

### Requirement: Relay Group Trust Boundary

Relay group hosting SHALL remain within the existing local runtime trust
boundary:

- same-user ownership checks for runtime artifacts,
- same-host local socket assumptions,
- no new remote control surface.

#### Scenario: Enforce existing ownership checks for group-selected bundles

- **WHEN** `agentmux host relay --group dev` initializes runtime artifacts for
  selected bundles
- **THEN** ownership and permission checks remain enforced per bundle
- **AND** foreign-owned runtime artifacts are rejected
