## MODIFIED Requirements

### Requirement: Send Target Selection

Send target identifiers SHALL be:

- bundle member session id
- UI session id (where UI routing is supported)

Configured session `name` values and display-name aliases are not canonical send
target identifiers and SHALL NOT be relay-routed.

If one token matches both a bundle member `session_id` and UI session id, the
bundle member `session_id` interpretation SHALL win.

`send` SHALL NOT accept any transport-scoped timeout override
field in v1. v1 of ACP, Tmux, and Pty delivery is fully
config-only: the per-coder config keys
`[coders.<id>.acp].prime-timeout-ms`,
`[coders.<id>.pty].prime-timeout-ms`,
`[coders.<id>.tmux].prime-timeout-ms`, and
`[coders.<id>.tmux].readiness-timeout-ms` are the only timeout
surfaces.

Adding a per-coder timeout key SHALL NOT be read as licence to add a per-call
override for it. The config-only property is the invariant this requirement
states; the enumeration of keys is incidental to it and SHALL be kept current as
keys are added, so that "the only timeout surfaces" remains a true statement
rather than a stale one. The enumeration SHALL be reconciled against the
authoritative descriptor list in the `addressing-routing` capability's `Bundle
Membership Configuration` requirement rather than extended only with the key a
change happens to introduce. Reconciling it here restored
`[coders.<id>.pty].prime-timeout-ms`, which shipped with `add-pty-transport` and
was never added to this list.

The pre-existing `quiescence_timeout_ms` payload field was
retired by the `tmux-wedge-detection` proposal; the pre-existing
`acp_turn_timeout_ms` payload field is retired by this proposal.
Both fields are rejected by the MCP server as unknown. Alpha
defaults apply: the rejection is a generic unknown-field error;
the server is NOT required to name a replacement. Operators who
hit the rejection consult the changelog.

`send` SHALL reject ACP timeout overrides against non-ACP targets
with `validation_invalid_timeout_field_for_transport`. With no
transport-scoped timeout override fields in v1, this validation
class is reserved for future per-call overrides (if/when a
transport-neutral `prime_timeout_ms` payload field is
reintroduced — see `design.md` Future Work).

`send` authorization scope SHALL follow requester policy control:

- `home`
- `all`

#### Scenario: Reject non-canonical configured-name token for explicit send target

- **WHEN** `send` targets a configured session `name` token
- **THEN** the tool returns `validation_unknown_target`

#### Scenario: Resolve overlap token as bundle member session_id

- **WHEN** one explicit target token matches both bundle member `session_id` and
  UI session id
- **THEN** the token is interpreted as bundle member `session_id`

#### Scenario: Reject retired tmux timeout payload field

- **WHEN** `send` request payload includes
  `quiescence_timeout_ms` (a field that does not exist in v1)
- **THEN** the tool rejects the field as unknown

#### Scenario: Reject retired ACP timeout payload field

- **WHEN** `send` request payload includes
  `acp_turn_timeout_ms` (a field that does not exist in v1)
- **THEN** the tool rejects the field as unknown

#### Scenario: Reject hypothetical ACP prime timeout payload field

- **WHEN** `send` request payload includes
  `acp_prime_timeout_ms` (a field that has never existed)
- **THEN** the tool rejects the field as unknown

#### Scenario: The readiness bound has no per-call payload field

- **WHEN** a caller wants to change how long a Tmux delivery waits for a target
  to become ready
- **THEN** the only surface is the per-coder
  `[coders.<id>.tmux].readiness-timeout-ms` config key
- **AND** `send` accepts no payload field to override it for one call
