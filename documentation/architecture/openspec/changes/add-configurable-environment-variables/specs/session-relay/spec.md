## ADDED Requirements

### Requirement: Coder Environment Variables

A `[[coders]]` entry SHALL support an optional `environment` array of entries
with required `name` and `value` string fields. The runtime SHALL apply the
coder's declared environment to the child process it spawns for a session
backed by that coder, regardless of which transport (Tmux, Pty, or ACP
command-spawn) the coder selects. Environment declared on a coder whose target
does not spawn a local child process (ACP `http` channel) SHALL be inert rather
than a load error.

This `environment` array is the sole coder-level environment surface; there is
no transport-scoped environment subtable.

#### Scenario: Coder environment applied to spawned child

- **WHEN** a `[[coders]]` entry declares `environment` entries
- **AND** a session references that coder over a spawning transport
- **THEN** the runtime sets those name/value pairs in the spawned child's
  environment

#### Scenario: Reject malformed coder environment entry

- **WHEN** a `[[coders]]` entry declares an `environment` entry missing `name`
  or `value`
- **THEN** relay rejects configuration with a structured config error

#### Scenario: Coder environment inert on non-spawning target

- **WHEN** a `[[coders]]` entry declares `environment`
- **AND** its target is an ACP `http` channel with no local child process
- **THEN** configuration loads successfully
- **AND** no environment is applied

### Requirement: Bundle Environment Variables

A per-bundle `bundles/<bundle-id>.toml` file SHALL support an optional
top-level `environment` array of `name`/`value` entries. The runtime SHALL
apply the bundle environment to the spawned child of every coder-backed session
in that bundle.

#### Scenario: Bundle environment applied to all bundle sessions

- **WHEN** a bundle file declares a top-level `environment` array
- **THEN** each coder-backed session spawned for that bundle receives those
  name/value pairs in its child environment

#### Scenario: Reject malformed bundle environment entry

- **WHEN** a bundle file declares an `environment` entry missing `name` or
  `value`
- **THEN** relay rejects configuration with a structured config error

### Requirement: Session Environment Variables

A coder-backed `[[sessions]]` entry SHALL support an optional `environment`
array of `name`/`value` entries. The runtime SHALL apply the session
environment to that session's spawned child.

#### Scenario: Session environment applied to its child

- **WHEN** a `[[sessions]]` entry declares an `environment` array
- **THEN** the runtime sets those name/value pairs in that session's spawned
  child environment

#### Scenario: Reject malformed session environment entry

- **WHEN** a `[[sessions]]` entry declares an `environment` entry missing
  `name` or `value`
- **THEN** relay rejects configuration with a structured config error

### Requirement: Environment Variable Precedence

The runtime SHALL resolve environment variables declared at more than one of
the coder, bundle, and session levels with per-variable precedence
**session > bundle > coder** (most-specific level wins), and SHALL union
variable names declared at only one level into the effective environment. The
runtime SHALL compute this merge once at configuration load and apply the
merged result at spawn.

#### Scenario: Session value overrides bundle and coder

- **WHEN** the coder, bundle, and session each declare the same variable name
  with different values
- **THEN** the spawned child receives the session-declared value

#### Scenario: Bundle value overrides coder

- **WHEN** the coder and bundle declare the same variable name with different
  values
- **AND** the session does not declare that name
- **THEN** the spawned child receives the bundle-declared value

#### Scenario: Distinct names union across levels

- **WHEN** the coder, bundle, and session each declare different variable names
- **THEN** the spawned child receives all of them
