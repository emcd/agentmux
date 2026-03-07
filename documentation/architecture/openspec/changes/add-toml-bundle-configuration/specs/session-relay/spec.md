## MODIFIED Requirements

### Requirement: Bundle Membership Configuration

The system SHALL let operators define bundle membership in per-bundle TOML
files with kebab-case keys:

- `bundles/<bundle-id>.toml`

Each bundle file SHALL include:

- `format-version`
- `[[sessions]]` entries with:
  - `id`
  - `name` (tmux routing session name)
  - optional `display-name`
  - `directory`
  - `coder`
  - optional `coder-session-id`

Routing and delivery SHALL continue to use session `name` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid TOML bundle configuration

- **WHEN** target `bundles/<bundle-id>.toml` contains unique session IDs and
  unique session names
- **AND** each session `coder` references an existing coder ID from
  `coders.toml`
- **THEN** the system loads the bundle definition successfully

#### Scenario: Reject unknown coder reference

- **WHEN** a session references a `coder` value not present in `coders.toml`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject duplicate session name in one bundle

- **WHEN** one bundle contains duplicate session `name` values
- **THEN** the system rejects configuration with a validation error

## ADDED Requirements

### Requirement: Coder Command Template Resolution

The system SHALL resolve per-session startup commands from referenced coder
templates in `coders.toml`.

Each coder definition SHALL include:

- `id`
- `initial-command`
- `resume-command`
- optional `prompt-regex`
- optional `prompt-inspect-lines`
- optional `prompt-idle-column`

Resolution SHALL follow:

1. If session `coder-session-id` is set, use coder `resume-command`.
2. Otherwise use coder `initial-command`.

Template placeholders SHALL be validated before reconciliation starts. Unknown
or unresolved placeholders SHALL fail configuration validation.

#### Scenario: Use resume command when coder-session-id is present

- **WHEN** a session includes `coder-session-id`
- **THEN** the system resolves startup command from coder `resume-command`
- **AND** substitutes `{coder-session-id}` with the session value

#### Scenario: Use initial command when coder-session-id is absent

- **WHEN** a session does not include `coder-session-id`
- **THEN** the system resolves startup command from coder `initial-command`

#### Scenario: Reject unresolved placeholder during validation

- **WHEN** a chosen command template requires placeholders not provided by the
  session definition
- **THEN** the system rejects configuration with a validation error

### Requirement: Coder-Scoped Prompt-Readiness Templates

The system SHALL allow prompt-readiness templates to be defined per coder.
Sessions that reference a coder inherit that coder's prompt-readiness settings.

#### Scenario: Apply prompt regex from referenced coder

- **WHEN** a session references a coder that defines `prompt-regex`
- **THEN** relay evaluates prompt readiness for that session using the coder
  template

#### Scenario: Use coder prompt inspect line setting when configured

- **WHEN** a coder defines `prompt-inspect-lines`
- **THEN** relay uses that value as the prompt-readiness inspection window for
  sessions that reference the coder

#### Scenario: Use coder prompt idle column when configured

- **WHEN** a coder defines `prompt-idle-column`
- **THEN** relay requires tmux `cursor_x` to match that value before injection
  for sessions that reference the coder
