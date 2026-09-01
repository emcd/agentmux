## MODIFIED Requirements

### Requirement: Session Type Taxonomy

The relay SHALL recognize exactly five session types, resolved from config:

| Type | Origin | Delivery binding | Notes |
|---|---|---|---|
| `tmux` | coder-backed; coder defines `[coders.tmux]` | tmux pane prompt injection + quiescence gating | MCP server socket; request/reply |
| `acp` | coder-backed; coder defines `[coders.acp]` | ACP prompt via relay-spawned worker | Bidirectional; relay drives channel |
| `pty` | coder-backed; coder defines `[coders.pty]` | native PTY write via libghostty-vt + portable-pty | Relay owns the child and its terminal |
| `ui` | coder-less `[sessions.ui]` marker | live relay stream push events | Bare marker subtable; no required fields |
| `pubsub` | coder-less `[sessions.pubsub]` marker | embedded callback; envelope as prompt | In-process tool calls |

A coder-backed session's type (`tmux`, `acp`, or `pty`) SHALL be derived from
the referenced coder's descriptor; the session entry SHALL NOT restate it. A
coder-less session's type (`ui` or `pubsub`) SHALL be its declared marker
subtable.

Session type SHALL be determined solely from config. Hello frames SHALL NOT
carry or assert session type.

`ui` and `pubsub` session types SHALL be recognized and validated from day one.
Sessions of these types SHALL be excluded from active routing at startup with a
structured `runtime_session_type_not_implemented` failure rather than a parse
error, until delivery is implemented.

#### Scenario: Derive tmux session type from referenced coder

- **WHEN** a session entry references a coder whose descriptor is
  `[coders.tmux]`
- **AND** the relay starts up
- **THEN** relay routes messages to that session via prompt injection

#### Scenario: Derive acp session type from referenced coder

- **WHEN** a session entry references a coder whose descriptor is
  `[coders.acp]`
- **THEN** relay delivers to that session via the ACP worker path

#### Scenario: Derive pty session type from referenced coder

- **WHEN** a session entry references a coder whose descriptor is
  `[coders.pty]`
- **THEN** relay delivers to that session via the Pty transport

#### Scenario: Fail fast for unimplemented session type

- **WHEN** a session entry declares a `[sessions.ui]` or `[sessions.pubsub]`
  marker subtable
- **THEN** relay emits `runtime_session_type_not_implemented` for that session
- **AND** excludes it from routing without aborting other session startup
