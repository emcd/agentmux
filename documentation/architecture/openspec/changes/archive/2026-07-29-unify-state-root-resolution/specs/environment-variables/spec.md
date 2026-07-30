## MODIFIED Requirements

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
