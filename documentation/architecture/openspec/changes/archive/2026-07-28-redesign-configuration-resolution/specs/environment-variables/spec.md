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

Within association resolution, the stamped context ranks **above** any
configuration file and below explicit invocation intent, because it carries what
bring-up authoritatively knows rather than a declared preference.

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
