## MODIFIED Requirements
### Requirement: Relay Host Bundle Selection

`agentmux host relay` SHALL require exactly one selector mode:

- positional `<bundle-id>`
- `--group <GROUP>`

`<bundle-id>` and `--group` SHALL be mutually exclusive.

#### Scenario: Host relay for one explicit bundle

- **WHEN** an operator runs `agentmux host relay <bundle-id>`
- **THEN** the system starts relay hosting flow for that bundle

#### Scenario: Host relay for reserved ALL group

- **WHEN** an operator runs `agentmux host relay --group ALL`
- **THEN** the system starts bundle-group relay hosting for all configured
  bundles

#### Scenario: Host relay for one custom group

- **WHEN** an operator runs `agentmux host relay --group dev`
- **THEN** the system starts bundle-group relay hosting for bundles assigned to
  group `dev`

#### Scenario: Reject missing selector mode

- **WHEN** an operator runs `agentmux host relay` without `<bundle-id>` and
  without `--group`
- **THEN** the system rejects invocation with a structured argument validation
  error

#### Scenario: Reject conflicting selector modes

- **WHEN** an operator runs `agentmux host relay <bundle-id> --group ALL`
- **THEN** the system rejects invocation with a structured argument validation
  error

## ADDED Requirements
### Requirement: Relay Host Startup Summary Contract

`agentmux host relay` SHALL expose a canonical machine startup summary payload.

The summary SHALL include:

- `schema_version`
- `host_mode` (`single_bundle`|`bundle_group`)
- `group_name` (optional, present only in group mode)
- `bundles` array with per-bundle entries:
  - `bundle_name`
  - `outcome` (`hosted`, `skipped`, `failed`)
  - `reason_code` (nullable)
  - `reason` (nullable human text)
- `hosted_bundle_count`
- `skipped_bundle_count`
- `failed_bundle_count`
- `hosted_any` (boolean)

When a bundle is skipped due to runtime lock contention, `reason_code` SHALL be
`lock_held`.

CLI text output SHALL be a rendering layer over the same summary payload.

#### Scenario: Emit startup summary payload in group mode

- **WHEN** relay host starts with `--group dev`
- **THEN** startup outcomes are represented in the canonical machine payload
- **AND** `host_mode` is `bundle_group`
- **AND** `group_name` is `dev`

#### Scenario: Emit startup summary payload in single-bundle mode

- **WHEN** relay host starts with positional `<bundle-id>`
- **THEN** startup outcomes are represented in the canonical machine payload
- **AND** `host_mode` is `single_bundle`
- **AND** `group_name` is omitted

#### Scenario: Mark lock-held skip outcome with canonical reason code

- **WHEN** one selected bundle is skipped because its runtime lock is held
- **THEN** the bundle entry uses `outcome=skipped`
- **AND** sets `reason_code=lock_held`

### Requirement: Relay Host CLI Scope (MVP)

MVP relay host group mode SHALL NOT support:

- `--all`
- `--include-bundle`
- `--exclude-bundle`

#### Scenario: Reject all flag in group-mode MVP

- **WHEN** an operator passes `--all` to `agentmux host relay`
- **THEN** the system rejects invocation with a structured argument validation
  error

#### Scenario: Reject include-bundle override in MVP

- **WHEN** an operator passes `--include-bundle` to `agentmux host relay`
- **THEN** the system rejects invocation with a structured argument validation
  error

#### Scenario: Reject exclude-bundle override in MVP

- **WHEN** an operator passes `--exclude-bundle` to `agentmux host relay`
- **THEN** the system rejects invocation with a structured argument validation
  error
