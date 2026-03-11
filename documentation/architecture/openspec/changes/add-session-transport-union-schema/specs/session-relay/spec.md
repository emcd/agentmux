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
  - optional `session-mode` (`new` | `load`, default `new`)
  - for `channel = "stdio"`:
    - required `command`
    - optional `args` (`string[]`)
    - optional `env` entries (`name`, `value`)
  - for `channel = "http"`:
    - required `url`
    - optional `headers` entries (`name`, `value`)

Session constraints:

- if referenced coder uses ACP with `session-mode = "load"`, session SHALL
  provide `coder-session-id`.

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid v2 tmux coder + session configuration

- **WHEN** bundle file uses `format-version = 2`
- **AND** coders file uses `format-version = 2`
- **AND** a coder defines `[coders.tmux]` with required fields
- **AND** a session references that coder
- **THEN** the system loads configuration successfully

#### Scenario: Load valid v2 ACP stdio coder + session configuration

- **WHEN** bundle and coders files use `format-version = 2`
- **AND** a coder defines `[coders.acp]`
- **AND** `coders.acp.channel = "stdio"`
- **AND** `coders.acp.command` is provided
- **AND** a session references that coder
- **THEN** the system loads configuration successfully

#### Scenario: Reject missing coder target descriptor

- **WHEN** a coder omits both `[coders.tmux]` and `[coders.acp]`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject multiple coder target descriptors

- **WHEN** a coder defines both `[coders.tmux]` and `[coders.acp]`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject ACP load mode without session identity

- **WHEN** a session references an ACP coder with `session-mode = "load"`
- **AND** the session omits `coder-session-id`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject ACP stdio channel without command

- **WHEN** a coder defines `[coders.acp]`
- **AND** `coders.acp.channel = "stdio"`
- **AND** `coders.acp.command` is missing
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject unsupported format-version

- **WHEN** bundle or coders file uses `format-version` other than `2`
- **THEN** the system rejects configuration with a validation error
