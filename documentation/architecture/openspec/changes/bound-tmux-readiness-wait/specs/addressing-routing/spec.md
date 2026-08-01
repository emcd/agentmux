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

- `[coders.tmux]` → Tmux-backed coder delivery (existing)
- `[coders.acp]` → ACP-backed coder delivery (existing)
- `[coders.pty]` → Pty-backed coder delivery via libghostty-vt + portable-pty
  (new in `add-pty-transport`)

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
  - optional `prime-timeout-ms`
  - optional `wedge-detection` (default `true`)
  - optional `readiness-timeout-ms` (default `900_000`, range
    `30_000..=3_600_000`)
- `[coders.acp]`:
  - required `channel` (`stdio` | `http`)
  - for `channel = "stdio"`: required `command`
  - for `channel = "http"`: required `url`; optional `headers` entries
    (`name`, `value`)
  - optional `prime-timeout-ms`
- `[coders.pty]` (new in `add-pty-transport`):
  - required `initial-command`
  - required `resume-command`
  - optional `prompt-regex`
  - optional `prompt-inspect-lines`
  - optional `prompt-idle-column`
  - optional `cols` (default 120) and `rows` (default 40)
  - optional `prime-timeout-ms`
  - optional `wedge-detection` (default `true`)
  - optional `term-protocol` (default `xterm-256color`)

This enumeration is authoritative for what an operator may write, so it SHALL be
kept complete as descriptor keys are added, and SHALL be reconciled against the
loader rather than extended only with the key a change happens to introduce.

Reconciling it here found four keys the loader has accepted while this list
omitted them: `prime-timeout-ms` and `wedge-detection` under `[coders.tmux]`
since `tmux-wedge-detection`, `prime-timeout-ms` under `[coders.acp]`, and
`term-protocol` under `[coders.pty]`. All four are restored alongside the new
`readiness-timeout-ms`. That four accumulated undetected is the reason the
reconciliation duty is stated rather than assumed.

ACP lifecycle selection constraints:

- if ACP-backed session includes `coder-session-id`, runtime SHALL call
  `session/load` for that session.
- if ACP-backed session omits `coder-session-id`, runtime SHALL call
  `session/new` for that session.
- if ACP `session/load` fails, runtime SHALL fail that session and SHALL NOT
  silently fall back to `session/new`.

Pty and Tmux lifecycle selection constraints:

- if a coder-backed session includes `coder-session-id` and the coder defines
  `[coders.pty]` (Pty) or `[coders.tmux]` (Tmux), the runtime SHALL construct
  the resume command by substituting `{coder-session-id}` into the
  `resume-command` template.
- if the coder-backed session omits `coder-session-id`, the runtime SHALL
  use the `initial-command` template.
- if the template substitution leaves an unresolved placeholder, the
  validator SHALL reject the configuration during load.

Routing and delivery SHALL use session `id` values.
Bundle identity SHALL be derived from bundle filename (`<bundle-id>.toml`).

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

#### Scenario: Accept a Tmux coder declaring a readiness timeout

- **WHEN** a `[[coders]]` entry defines `[coders.tmux]` with
  `readiness-timeout-ms` set to a value within the permitted range
- **THEN** the system loads configuration successfully
- **AND** the value governs the readiness bound for that coder's Tmux deliveries

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
