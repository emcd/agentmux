## MODIFIED Requirements

### Requirement: Send Target Mode Selection

`agentmux send` SHALL support exactly one target mode per request:

- one or more explicit `--target` values
- `--broadcast`

For explicit `--target` mode, tokens SHALL be canonical send target
identifiers only.
Configured session `name` values and display-name aliases are not valid
explicit send targets.

Send authorization SHALL follow requester policy control scope:

- `all:home`
- `all:all`

#### Scenario: Send to explicit targets

- **WHEN** a caller invokes `agentmux send` with `--target` values
- **THEN** the system routes to exactly those selected recipients

#### Scenario: Reject configured name alias token for send target

- **WHEN** a caller invokes `agentmux send --target <configured-session-name>`
- **THEN** CLI surfaces `validation_unknown_target`

#### Scenario: Reject conflicting target modes

- **WHEN** a caller provides both explicit `--target` values and `--broadcast`
- **THEN** the system rejects invocation with
  `validation_conflicting_targets`

#### Scenario: Deny cross-bundle send under home-only scope

- **WHEN** caller requests cross-bundle send
- **AND** requester policy `send` scope is `all:home`
- **THEN** CLI surfaces `authorization_forbidden`
