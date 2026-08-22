# environment-variables Specification

## Purpose

Coder/bundle/session environment variable precedence and container-injected overrides.
## Requirements
### Requirement: Coder Environment Variables

A `[[coders]]` entry SHALL support an optional `environment` array of entries
with required `name` and `value` string fields. Each `name` MUST be a usable OS
environment-variable key — non-empty and free of `=` and NUL — validated at
configuration load; a `value` MAY be an empty string. The runtime SHALL apply
the coder's declared environment to the child process it spawns for a session
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

- **WHEN** a `[[coders]]` entry declares an `environment` entry whose `name` or
  `value` field is missing, or whose `name` is empty or contains `=` or NUL
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

- **WHEN** a bundle file declares an `environment` entry whose `name` or
  `value` field is missing, or whose `name` is empty or contains `=` or NUL
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

- **WHEN** a `[[sessions]]` entry declares an `environment` entry whose `name`
  or `value` field is missing, or whose `name` is empty or contains `=` or NUL
- **THEN** relay rejects configuration with a structured config error

### Requirement: Environment Variable Precedence

The runtime SHALL resolve environment variables declared at more than one of
the coder, bundle, and session levels with per-variable precedence
**session > bundle > coder** (most-specific level wins), and SHALL union
variable names declared at only one level into the effective environment. The
runtime SHALL compute this merge once at configuration load and apply the
merged result at spawn.

After merging the operator-declared layers, configuration load SHALL stamp
authoritative bring-up context onto a coder-backed member's effective
environment upsert-if-absent, so an operator-declared entry of the same name is
preserved (operator-declared wins). The stamped context, its extensibility, and
its consumption during MCP association resolution are specified by the
runtime-bootstrap spec's Bring-Up Association Environment Injection requirement.

The relay's effective configuration layer list SHALL be stamped as
`AGENTMUX_CONFIGURATION_DIRECTORY` under these ordinary upsert-if-absent rules,
so a member reads the declarations of the relay that spawned it rather than
resolving its own configuration root independently.

`AGENTMUX_STATE_DIRECTORY` is the one exception, and it SHALL be injected at
spawn time by the relay, authoritatively, overwriting any operator-declared
value at any level.

The exception is narrow and follows from what the variable addresses rather than
from a preference about precedence. Every other stamped variable describes an
identity a member may legitimately want to assert; this one names the relay the
member is a child of. An operator-declared value would not override a
preference, it would break the rendezvous — the child would address a relay that
did not spawn it, while the relay that did waits for a client that never arrives.
There is no legitimate case for the override, because a member of one relay
reaching another is expressed by configured peers rather than by re-pointing a
child.

The value is also unavailable at configuration load: it belongs to the relay
performing the spawn, not to the configuration being loaded.

`AGENTMUX_CONFIGURATION_DIRECTORY` SHALL NOT be admitted to that exception, and
the distinction SHALL NOT be described as a rendezvous concern. The socket,
session and peer pre-shared keys, and the principal store all resolve beneath the
state root, so a member holding a divergent configuration root still addresses
and authenticates to the relay that spawned it; what diverges is the set of
declarations it reads. Bundle and session context already outrank configuration
in association resolution, so the divergence does not misidentify the member
either. An authoritative injection would additionally be unenforceable, because
the environment tier of configuration resolution ranks below the command-line
flag: a declared `--configuration-directory` outranks any stamped value.

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

#### Scenario: Operator-declared context variable is preserved

- **WHEN** a coder-backed member explicitly declares `AGENTMUX_BUNDLE`
- **THEN** configuration load preserves the operator-declared value rather than
  overwriting it with the stamped context

#### Scenario: Operator-declared state directory is overwritten

- **WHEN** a coder, bundle, or member declares `AGENTMUX_STATE_DIRECTORY`
- **THEN** the relay's normalized state root is injected in its place at spawn
- **AND** the child addresses the relay that spawned it

#### Scenario: Configuration directory is stamped when undeclared

- **WHEN** a coder-backed member declares no `AGENTMUX_CONFIGURATION_DIRECTORY`
- **THEN** configuration load stamps the relay's effective configuration layer
  list into the member's spawn environment

#### Scenario: Operator-declared configuration directory is preserved

- **WHEN** a coder, bundle, or member declares `AGENTMUX_CONFIGURATION_DIRECTORY`
- **THEN** configuration load leaves that entry's value untouched
- **AND** the stamp does not overwrite it, unlike the state-directory exception

