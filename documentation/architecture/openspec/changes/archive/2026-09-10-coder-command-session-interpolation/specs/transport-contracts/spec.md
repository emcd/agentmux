## MODIFIED Requirements

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

Rendering applies to tmux `initial-command`/`resume-command` and pty
`initial-command`/`resume-command`. ACP stdio `command` is a verbatim
passthrough and is never rendered.

The template vocabulary SHALL be three double-brace names only. The
placeholder scanner detects both `{name}` and `{{name}}` shapes, but
every occurrence whose name does not match `{{coder-session-id}}`,
`{{bundle-session-id}}`, or `{{session-directory}}` — including every
single-brace occurrence — SHALL fail configuration validation as an
unknown placeholder.

- `{{coder-session-id}}` — the session's `coder-session-id`; required when
  present in the chosen template.
- `{{bundle-session-id}}` — the bare session id from the `[[sessions]]` table in
  `bundle.toml`, normalized as the canonical member id (not
  bundle-qualified).
- `{{session-directory}}` — the session's declared `directory`, rendered as
  a single shell-quoted word denoting that directory (POSIX single-quote
  escaping), so it arrives as one argument on both the tmux shell handoff
  and the pty `shell_words` handoff. It SHALL occupy an entire unquoted
  shell word in the original template (see below); the id tokens above
  substitute as raw values and may occur in any template context.

Template placeholders SHALL be validated before reconciliation starts, by
inspecting and classifying every placeholder occurrence in the original
template before substitution: each `{name}` or `{{name}}` (where `name`
matches `[a-z][a-z0-9_-]*`) is either a known variable or unknown.
Unknown occurrences SHALL fail configuration validation, as SHALL a
template using `{{coder-session-id}}` without a session value. Only
validated known occurrences are substituted; substituted value bytes
SHALL never be rescanned.

The placeholder `{{session-directory}}` SHALL be bounded on both sides
by a POSIX shell word boundary: the scanner SHALL be in Outside quote
state with no active escape at the opening `{{`, and the byte after the
closing `}}` SHALL be end-of-template or an unquoted unescaped
whitespace. Templates placing it inside single quotes, inside double
quotes, adjacent to non-boundary characters on either side, or after an
escape sequence that continues the current word SHALL fail configuration
validation.

#### Scenario: Use resume command when coder-session-id is present

- **WHEN** a session includes `coder-session-id`
- **THEN** the system resolves startup command from coder `resume-command`
- **AND** substitutes `{{coder-session-id}}` with the session value

#### Scenario: Use initial command when coder-session-id is absent

- **WHEN** a session does not include `coder-session-id`
- **THEN** the system resolves startup command from coder `initial-command`

#### Scenario: Substitute session id and directory placeholders

- **WHEN** a chosen command template contains `{{bundle-session-id}}` or
  `{{session-directory}}`
- **THEN** the system substitutes the session id and the session's declared
  directory respectively
- **AND** the directory arrives as one shell word on both transports

#### Scenario: Resolve brace-shaped directory without false rejection

- **WHEN** a chosen command template contains `{{session-directory}}`
- **AND** the session's declared directory contains brace-shaped text (e.g.
  `/work/{name}`)
- **THEN** the system resolves the command successfully
- **AND** the brace-shaped value bytes are not rejected as placeholders

#### Scenario: Preserve directory with command-significant characters

- **WHEN** a chosen command template contains `{{session-directory}}`
- **AND** the session's declared directory contains spaces, quotes, or
  backslashes
- **THEN** the system resolves the command with the directory as a single
  quoted word denoting the declared directory

#### Scenario: Reject unknown placeholder despite brace-shaped directory

- **WHEN** a chosen command template contains an unknown `{{name}}`
  placeholder
- **AND** the session's declared directory contains brace-shaped text
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject unknown double-brace placeholder during validation

- **WHEN** a chosen command template contains a `{{name}}` placeholder
  outside the three-name vocabulary (`{{coder-session-id}}`,
  `{{bundle-session-id}}`, `{{session-directory}}`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject single-brace coder-session-id as unknown

- **WHEN** a chosen command template contains `{coder-session-id}` in
  single-brace form
- **THEN** the system rejects configuration with a validation error
  naming it an unknown placeholder

#### Scenario: Accept session-directory as a standalone unquoted word

- **WHEN** a chosen command template contains `{{session-directory}}` as
  an entire unquoted shell word (bounded on both sides by a word
  boundary in the original template)
- **THEN** the system resolves the command with the directory as a single
  quoted word denoting the declared directory

#### Scenario: Reject session-directory inside single quotes

- **WHEN** a chosen command template contains `{{session-directory}}`
  inside single quotes
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory inside double quotes

- **WHEN** a chosen command template contains `{{session-directory}}`
  inside double quotes
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory with an adjacent prefix

- **WHEN** a chosen command template contains `{{session-directory}}`
  immediately preceded by a non-boundary character
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory with an adjacent suffix

- **WHEN** a chosen command template contains `{{session-directory}}`
  immediately followed by a non-boundary character
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory after an escape continuing the word

- **WHEN** a chosen command template contains `{{session-directory}}`
  immediately preceded by an escape sequence that continues the current
  word
- **THEN** the system rejects configuration with a validation error

#### Scenario: Accept session-directory after escape-then-whitespace

- **WHEN** a chosen command template contains an escaped character, then
  an unquoted unescaped whitespace, then `{{session-directory}}` as a
  standalone word
- **THEN** the system resolves the command with the directory as a single
  quoted word denoting the declared directory

#### Scenario: Reject unresolved placeholder during validation

- **WHEN** a chosen command template requires placeholders not provided by the
  session definition
- **THEN** the system rejects configuration with a validation error
