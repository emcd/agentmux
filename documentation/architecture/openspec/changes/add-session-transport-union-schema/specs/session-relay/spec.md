## MODIFIED Requirements

### Requirement: Bundle Membership Configuration

The system SHALL let operators define bundle membership in per-bundle TOML
files with kebab-case keys:

- `bundles/<bundle-id>.toml`

Each bundle file SHALL include:

- `format-version` (supported values: `1` and `2`)
- `[[sessions]]` entries with:
  - `id`
  - optional `name` (human-readable recipient name)
  - `directory`

Session transport schema SHALL follow format-version rules:

- `format-version = 1` (legacy tmux schema):
  - required `coder`
  - optional `coder-session-id`
  - implicit transport kind `tmux`
- `format-version = 2` (transport-aware schema):
  - optional `[sessions.transport]` table
  - `sessions.transport.kind` allowed values: `tmux`, `acp`
  - omitted `sessions.transport` defaults to `kind = "tmux"`
  - for `kind = "tmux"`:
    - required `coder`
    - optional `coder-session-id`
  - for `kind = "acp"`:
    - required `[sessions.transport.acp]` table
    - required `transport` (`stdio` | `http`)
    - required `session_mode` (`new` | `load`)
    - required `session_id` when `session_mode = "load"`
    - for `transport = "stdio"`:
      - required `command`
      - optional `args` (`string[]`)
      - optional `env` entries (`name`, `value`)
    - for `transport = "http"`:
      - required `url`
      - optional `headers` entries (`name`, `value`)

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

#### Scenario: Load valid v1 tmux bundle configuration

- **WHEN** target `bundles/<bundle-id>.toml` uses `format-version = 1`
- **AND** each `[[sessions]]` entry includes `coder`
- **THEN** the system loads bundle configuration with implicit tmux transport

#### Scenario: Load valid v2 bundle with implicit tmux transport

- **WHEN** target `bundles/<bundle-id>.toml` uses `format-version = 2`
- **AND** a `[[sessions]]` entry omits `[sessions.transport]`
- **AND** that session includes `coder`
- **THEN** the system loads that session as `kind = "tmux"`

#### Scenario: Load valid v2 ACP stdio session configuration

- **WHEN** target `bundles/<bundle-id>.toml` uses `format-version = 2`
- **AND** a `[[sessions]]` entry sets `sessions.transport.kind = "acp"`
- **AND** `sessions.transport.acp.transport = "stdio"`
- **AND** `sessions.transport.acp.command` is provided
- **AND** `sessions.transport.acp.session_mode = "new"`
- **THEN** the system loads the ACP session configuration successfully

#### Scenario: Reject unknown transport kind in v2

- **WHEN** a v2 session sets `sessions.transport.kind` to a value other than
  `tmux` or `acp`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject ACP load mode without session_id

- **WHEN** a v2 session sets `sessions.transport.kind = "acp"`
- **AND** `sessions.transport.acp.session_mode = "load"`
- **AND** `sessions.transport.acp.session_id` is missing
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject ACP stdio transport without command

- **WHEN** a v2 session sets `sessions.transport.kind = "acp"`
- **AND** `sessions.transport.acp.transport = "stdio"`
- **AND** `sessions.transport.acp.command` is missing
- **THEN** the system rejects configuration with a validation error

## ADDED Requirements

### Requirement: Session Schema Migration Compatibility

The system SHALL support staged migration from tmux-only bundles to
transport-aware bundles without breaking existing valid `format-version = 1`
configurations.

#### Scenario: Preserve legacy tmux-only bundle behavior

- **WHEN** an existing bundle file remains on `format-version = 1`
- **THEN** session loading behavior remains compatible with current tmux-only
  semantics

#### Scenario: Allow mixed migration at repository scope

- **WHEN** one bundle file uses `format-version = 1`
- **AND** another bundle file uses `format-version = 2`
- **THEN** the system accepts both files in the same configuration root

#### Scenario: Reject unsupported format-version value

- **WHEN** a bundle file uses `format-version` outside supported values
- **THEN** the system rejects configuration with a validation error
