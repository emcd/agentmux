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

The template vocabulary SHALL be four double-brace names only. The
placeholder scanner detects both `{name}` and `{{name}}` shapes, but every
occurrence whose name does not match `{{coder-session-id}}`,
`{{bundle-session-id}}`, `{{session-directory}}`, or `{{project-name}}`
— including every single-brace occurrence — SHALL fail configuration
validation as an unknown placeholder.

- `{{coder-session-id}}` — the session's `coder-session-id`; required when
  present in the chosen template.
- `{{bundle-session-id}}` — the bare session id from the `[[sessions]]` table
  in `bundle.toml`, normalized as the canonical member id (not
  bundle-qualified).
- `{{session-directory}}` — the session's declared `directory`, rendered as a
  single shell-quoted word denoting that directory (POSIX single-quote
  escaping), so it arrives as one argument on both the tmux shell handoff and
  the pty `shell_words` handoff. It SHALL occur in an unquoted word in the
  original template (see below); the id and project tokens below substitute
  as raw values and may occur in any template context.
- `{{project-name}}` — the resolved project name for the session,
  substituted raw. Project names admit ASCII alphanumerics plus `-`,
  `_`, and `.`, excluding the exact `.` and `..` segments, with no
  length cap and no first-character restriction — a strict superset
  of the path-safe canonical bundle ids (`is_valid_bundle_name`
  minus the exact `./..` segments) — so no shell metacharacter,
  quote, or whitespace can reach a template from any resolution
  source.

Template placeholders SHALL be validated before reconciliation starts, by
inspecting and classifying every placeholder occurrence in the original
template before substitution: each `{name}` or `{{name}}` (where `name`
matches `[a-z][a-z0-9_-]*`) is either a known variable or unknown.
Unknown occurrences SHALL fail configuration validation, as SHALL a
template using `{{coder-session-id}}` without a session value. Only
validated known occurrences are substituted; substituted value bytes
SHALL never be rescanned.

The placeholder `{{session-directory}}` SHALL occur in an unquoted word
whose every literal affix character belongs to the shell-literal-safe
set (ASCII letters and digits plus `-_.:/=,+@%`) and whose every
adjacent placeholder is a grammar-safe variable
(`{{bundle-session-id}}`, `{{project-name}}`, or another quoted
`{{session-directory}}` — never `{{coder-session-id}}`, whose
unconstrained value could inject word-breaking bytes): the scanner SHALL be in
Outside quote state with no active escape at the opening `{{`, the
byte after the closing `}}` SHALL NOT be a quote character or an
escape, and for every `=` in the placeholder's word the word text
from its start through that `=`, read with each placeholder span
as one ASCII letter since substituted values are dynamic, SHALL
NOT match shell assignment shape (`^[A-Za-z_][A-Za-z0-9_]*=`). Templates placing it
inside single quotes, inside double quotes, after an escape sequence
that continues the current word, immediately adjacent to a quote
character or escape on either side, affixed with any other character,
or in a word that could form a shell assignment SHALL fail
configuration validation.

Project names SHALL resolve in precedence order: the session
`project-name` explicit override; else the applicable
`project-name-from` rule (session-level beats bundle-level), where
`session-directory-basename` yields the final segment of the declared
session directory and `bundle-name` yields the canonical bundle id;
else the session-directory basename. Derived values (rule yields and
the basename fallback) SHALL be resolved and grammar-validated only
when the chosen tmux/Pty template actually uses `{{project-name}}`;
explicit `project-name` values and `project-name-from` rule names
SHALL validate eagerly. A session declaring both its own
`project-name` and `project-name-from` is ambiguous and SHALL fail
configuration validation; a session `project-name` above a bundle-only
`project-name-from` wins by precedence. Every eagerly validated value
and every lazily derived value that is used SHALL satisfy the
project-name grammar; a non-conforming used value SHALL fail
configuration validation rather than sanitizing silently. Sessions
whose chosen template never mentions `{{project-name}}` — including
coder-less sessions, which have no command template — SHALL load
without any basename conformance requirement.

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

#### Scenario: Substitute project-name raw

- **WHEN** a chosen command template contains `{{project-name}}`
- **THEN** the system substitutes the resolved project name without
  quoting
- **AND** the value contains no shell metacharacters by grammar

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
  outside the four-name vocabulary (`{{coder-session-id}}`,
  `{{bundle-session-id}}`, `{{session-directory}}`, `{{project-name}}`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject single-brace coder-session-id as unknown

- **WHEN** a chosen command template contains `{coder-session-id}` in
  single-brace form
- **THEN** the system rejects configuration with a validation error naming it
  an unknown placeholder

#### Scenario: Accept session-directory composed in an unquoted word

- **WHEN** a chosen command template contains `{{session-directory}}`
  composed with literal affixes or adjacent variables in one unquoted
  word (e.g. `--dir={{session-directory}}`,
  `--mount {{project-name}}:{{session-directory}}`)
- **THEN** the system resolves the command with the composed word
  denoting the declared directory among the operator's affixes
- **AND** the composed word arrives as one shell word on both transports

#### Scenario: Reject session-directory with glob affix

- **WHEN** a chosen command template contains `{{session-directory}}`
  affixed with a glob character (e.g. `{{session-directory}}*`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory with expansion affix

- **WHEN** a chosen command template contains `{{session-directory}}`
  affixed with a shell expansion (e.g. `x${IFS}{{session-directory}}`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory with operator affix

- **WHEN** a chosen command template contains `{{session-directory}}`
  affixed with a shell operator (e.g. `{{session-directory}};next`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory with assignment prefix

- **WHEN** a chosen command template places `{{session-directory}}`
  in a word whose literal prefix from word start matches shell
  assignment shape (e.g. `A={{session-directory}}`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject variable composing assignment shape before equals

- **WHEN** a chosen command template places a placeholder where the
  word text from word start through a following `=`, read with each
  placeholder span as one ASCII letter, matches shell assignment
  shape (e.g. `{{project-name}}={{session-directory}}` resolving
  `project-name` to `ROOT`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject directory composing assignment shape before equals

- **WHEN** a chosen command template places `{{session-directory}}`
  where the word text from word start through a following `=`, read
  with the placeholder span as one ASCII letter, matches shell
  assignment shape (e.g. `A{{session-directory}}=x`)
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory before an escape

- **WHEN** a chosen command template contains `{{session-directory}}`
  immediately followed by an escape character
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory adjacent to coder-session-id

- **WHEN** a chosen command template places `{{session-directory}}`
  in one word adjacent to `{{coder-session-id}}`
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory inside single quotes

- **WHEN** a chosen command template contains `{{session-directory}}` inside
  single quotes
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory inside double quotes

- **WHEN** a chosen command template contains `{{session-directory}}` inside
  double quotes
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory quote-adjacent

- **WHEN** a chosen command template places `{{session-directory}}`
  immediately adjacent to a quote character on either side
- **THEN** the system rejects configuration with a validation error

#### Scenario: Reject session-directory after an escape continuing the word

- **WHEN** a chosen command template contains `{{session-directory}}`
  immediately preceded by an escape sequence that continues the current word
- **THEN** the system rejects configuration with a validation error

#### Scenario: Accept session-directory after escape-then-whitespace

- **WHEN** a chosen command template contains an escaped character, then an
  unquoted unescaped whitespace, then `{{session-directory}}` as a standalone
  word
- **THEN** the system resolves the command with the directory as a single
  quoted word denoting the declared directory

#### Scenario: Resolve project-name from session override

- **WHEN** a session declares `project-name`
- **THEN** the system resolves `{{project-name}}` to that value
  regardless of any bundle `project-name-from` rule

#### Scenario: Resolve project-name from session override above bundle rule

- **WHEN** a session declares `project-name`
- **AND** the session omits `project-name-from`
- **AND** the bundle declares a `project-name-from`
- **THEN** the system resolves `{{project-name}}` to the session value
- **AND** no ambiguity error surfaces

#### Scenario: Resolve project-name from session rule above bundle rule

- **WHEN** a session declares `project-name-from`
- **AND** its bundle declares a different `project-name-from`
- **THEN** the system resolves `{{project-name}}` per the session rule

#### Scenario: Resolve project-name from bundle rule

- **WHEN** a session omits `project-name`
- **AND** its bundle declares `project-name-from = "bundle-name"`
- **THEN** the system resolves `{{project-name}}` to the canonical
  bundle id

#### Scenario: Resolve project-name from dotted bundle-name

- **WHEN** a session omits `project-name`
- **AND** its bundle declares `project-name-from = "bundle-name"`
- **AND** the canonical bundle id contains dots (e.g. `team.one`)
- **THEN** the system resolves `{{project-name}}` to the dotted id

#### Scenario: Resolve project-name from directory basename by default

- **WHEN** a session omits `project-name`
- **AND** no applicable `project-name-from` rule names another source
- **THEN** the system resolves `{{project-name}}` to the final segment
  of the declared session directory

#### Scenario: Load session with nonconforming basename and no project-name usage

- **WHEN** a session declares a directory whose basename violates the
  project-name grammar (e.g. dotted, spaced, or hidden)
- **AND** no chosen template uses `{{project-name}}`
- **THEN** the system loads the configuration successfully
- **AND** no basename conformance is required

#### Scenario: Reject session declaring both project-name and project-name-from

- **WHEN** a session declares its own `project-name` and its own
  `project-name-from`
- **THEN** the system rejects configuration with a validation error
  naming the ambiguity

#### Scenario: Reject non-conforming project-name value

- **WHEN** a project-name resolution source yields a value outside the
  portable path-segment grammar
- **THEN** the system rejects configuration with a validation error
  rather than sanitizing the value

#### Scenario: Reject unresolved placeholder during validation

- **WHEN** a chosen command template requires placeholders not provided by the
  session definition
- **THEN** the system rejects configuration with a validation error
