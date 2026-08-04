## MODIFIED Requirements

### Requirement: Bundle Membership Configuration

The system SHALL let operators define bundle membership in per-bundle TOML
files with kebab-case keys:

- `bundles/<bundle-id>.toml`

Each bundle file SHALL include:

- `format-version` (supported value for this schema: `1`)
- `[[sessions]]` entries with:
  - `id`
  - optional `name` (human-readable recipient name)
  - `directory`
  - exactly one session shape: a coder-backed shape (a flat `coder` reference,
    with optional `coder-session-id`) or a coder-less shape (exactly one
    `[sessions.ui]` or `[sessions.pubsub]` marker subtable)

Session membership invariants SHALL remain enforced:

- session `id` values are unique within one bundle
- optional session `name` values are unique within one bundle when present

A coder-backed `[[sessions]]` entry SHALL carry:

- required `coder` reference (must resolve to a `[[coders]]` entry)
- optional `coder-session-id`

The session's transport SHALL be derived from the referenced coder's
descriptor:

- `[coders.tmux]` → Tmux-backed coder delivery
- `[coders.acp]` → ACP-backed coder delivery
- `[coders.pty]` → Pty-backed coder delivery via libghostty-vt + portable-pty

The session entry SHALL NOT restate the transport; the coder descriptor is
authoritative.

A coder-less `[[sessions]]` entry SHALL declare exactly one `[sessions.ui]` or
`[sessions.pubsub]` marker subtable, which SHALL carry no required fields
(empty body is valid). A coder-less entry SHALL NOT carry a `coder` or
`coder-session-id` field.

Coder definitions SHALL include target descriptors in `coders.toml`:

- `format-version` (supported value for this schema: `1`)
- `[[coders]]` entries with:
  - `id`
  - exactly one target descriptor table:
    - `[coders.tmux]`
    - `[coders.acp]`
    - `[coders.pty]`

Descriptor fields SHALL be:

- `[coders.tmux]`:
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
- `[coders.acp]`:
  - required `channel` (`stdio` | `http`)
  - for `channel = "stdio"`: required `command`
  - for `channel = "http"`: required `url`; optional `headers` entries
    (`name`, `value`)
- `[coders.pty]`:
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
  - optional `cols` (default 120) and `rows` (default 40)
  - optional `term-protocol` (default `xterm-256color`)

This enumeration is authoritative for what an operator may write, so it SHALL be
kept complete as descriptor keys are added, and SHALL be reconciled against the
loader rather than extended only with the key a change happens to introduce.

**No per-coder descriptor carries a delivery timeout.** How long a delivery may
wait is a property of the relay's patience, not of any coder, and is configured
relay-side per the `runtime-bootstrap` capability's `Relay Configuration File`
requirement. The prompt-readiness keys above remain per-coder because a prompt
frame genuinely is a property of the coder.

#### Scenario: Load valid tmux-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference
- **AND** the referenced coder defines `[coders.tmux]`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the tmux transport

#### Scenario: Load valid ACP-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference with
  `coder-session-id`
- **AND** the referenced coder defines `[coders.acp]` with `channel = "stdio"`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the ACP transport

#### Scenario: Load valid Pty-backed session configuration

- **WHEN** bundle and coders files use `format-version = 1`
- **AND** a session entry declares a flat `coder` reference
- **AND** the referenced coder defines `[coders.pty]` with
  `initial-command` and `resume-command`
- **THEN** the system loads configuration successfully
- **AND** the session is routed via the Pty transport
- **AND** the Pty transport spawns the child under a portable-pty master
  sized to the per-coder `cols` and `rows` (defaults 120 x 40)

#### Scenario: Reject session with neither coder nor marker

- **WHEN** a bundle session entry declares no `coder` reference and no
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject session declaring both coder and marker

- **WHEN** a bundle session entry declares a `coder` reference and also a
  `[sessions.ui]` or `[sessions.pubsub]` marker subtable
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Reject coder declaring both Pty and Tmux target descriptors

- **WHEN** a `[[coders]]` entry declares both `[coders.pty]` and
  `[coders.tmux]` subtables
- **THEN** the validator rejects the configuration with a structured config
  error
