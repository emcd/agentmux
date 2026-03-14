## MODIFIED Requirements

### Requirement: Bundle Membership Configuration

The system SHALL let operators define bundle membership in per-bundle TOML
files with kebab-case keys:

- `bundles/<bundle-id>.toml`

Each bundle file SHALL include:

- `format-version` (supported value for this schema: `2`)
- `[[sessions]]` entries with:
  - `id`
  - optional `name` (human-readable recipient name)
  - `directory`
  - required `coder` reference
  - optional `coder-session-id`

Session membership invariants SHALL remain enforced:

- session `id` values are unique within one bundle
- optional session `name` values are unique within one bundle when present
- each session `coder` references an existing coder id from `coders.toml`

Coder definitions SHALL include target descriptors in `coders.toml`:

- `format-version` (supported value for this schema: `2`)
- `[[coders]]` entries with:
  - `id`
  - exactly one target descriptor table:
    - `[coders.tmux]`
    - `[coders.acp]`

Descriptor fields SHALL be:

- `[coders.tmux]`:
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
- `[coders.acp]`:
  - required `channel` (`stdio` | `http`)
  - for `channel = "stdio"`:
    - required `command`
  - for `channel = "http"`:
    - required `url`
    - optional `headers` entries (`name`, `value`)

ACP lifecycle selection constraints:

- if ACP-backed session includes `coder-session-id`, runtime SHALL call
  `session/load` for that session.
- if ACP-backed session omits `coder-session-id`, runtime SHALL call
  `session/new` for that session.
- if ACP `session/load` fails, runtime SHALL fail that session operation and
  SHALL NOT silently fall back to ACP `session/new` in the same operation.

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid v2 tmux coder + session configuration

- **WHEN** bundle file uses `format-version = 2`
- **AND** coders file uses `format-version = 2`
- **AND** a coder defines `[coders.tmux]` with required fields
- **AND** sessions use unique `id` values
- **AND** optional session `name` values are unique when present
- **AND** each session references an existing coder
- **THEN** the system loads configuration successfully

#### Scenario: Load valid v2 ACP stdio coder + session configuration

- **WHEN** bundle and coders files use `format-version = 2`
- **AND** a coder defines `[coders.acp]`
- **AND** `coders.acp.channel = "stdio"`
- **AND** `coders.acp.command` is provided
- **AND** sessions use unique `id` values
- **AND** optional session `name` values are unique when present
- **AND** each session references an existing coder
- **THEN** the system loads configuration successfully

#### Scenario: Reject unknown coder reference

- **WHEN** a session references a `coder` value not present in `coders.toml`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject duplicate session id in one bundle

- **WHEN** one bundle contains duplicate session `id` values
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject duplicate session name in one bundle

- **WHEN** one bundle contains duplicate session `name` values
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject missing coder target descriptor

- **WHEN** a coder omits both `[coders.tmux]` and `[coders.acp]`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject multiple coder target descriptors

- **WHEN** a coder defines both `[coders.tmux]` and `[coders.acp]`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Select ACP session load when session identity is present

- **WHEN** a session references an ACP coder
- **AND** the session includes `coder-session-id`
- **THEN** runtime selects ACP `session/load` for that session.

#### Scenario: Fail fast when ACP session load fails

- **WHEN** runtime selects ACP `session/load` for a session
- **AND** the ACP `session/load` call returns an error
- **THEN** runtime fails the session operation
- **AND** runtime does not call ACP `session/new` as fallback in the same
  operation.

#### Scenario: Reject ACP stdio channel without command

- **WHEN** a coder defines `[coders.acp]`
- **AND** `coders.acp.channel = "stdio"`
- **AND** `coders.acp.command` is missing
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject unsupported format-version

- **WHEN** bundle or coders file uses `format-version` other than `2`
- **THEN** the system rejects configuration with a validation error
