## MODIFIED Requirements

### Requirement: MCP raww request contract

MCP `raww` request fields SHALL be:
- `target_session` (required)
- `text` (required)
- `mode` (optional string, one of `normal` or `emergency`, default `normal`)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)

`mode` SHALL be declared in the tool schema as a plain optional string with an
enumerated value set, not as a nullable union type.

`mode = "emergency"` requests the ordering break defined by the
`transport-contracts` capability's `Relay raww operation contract`: the write
overtakes that target's pending mail and bypasses the prompt-readiness gate. It
is supported on Tmux and Pty targets only, and MCP SHALL surface the relay's
rejection unchanged for any other transport.

Omitting `mode` SHALL produce exactly the behavior an MCP `raww` call had before
this change, so no existing caller changes meaning.

A bare `target_session` SHALL be qualified to the MCP server's bound bundle
before the relay call; a target that already carries an `@<namespace>` suffix is
forwarded verbatim. A `raww` call on an unassociated server SHALL be rejected
with `validation_unassociated_server` (per the Error Object Contract) before
qualification runs. Routing context for `raww` SHALL then be inferred from the
target's `@<namespace>` suffix. No explicit `namespace` parameter is accepted.

`raww` requests SHALL reject caller-supplied sender-like identity fields with
`validation_invalid_params`.

#### Scenario: Reject sender-like field in raww request

- **WHEN** caller submits `raww` request containing sender-like field
- **THEN** MCP rejects request with `validation_invalid_params`

#### Scenario: Default raww mode is normal

- **WHEN** caller omits `mode`
- **THEN** MCP forwards the relay request with `mode = "normal"`

#### Scenario: Forward emergency raww mode

- **WHEN** caller submits `raww` with `mode = "emergency"`
- **THEN** MCP forwards the relay request with `mode = "emergency"`

#### Scenario: Surface unsupported-transport rejection for emergency mode

- **WHEN** caller submits `raww` with `mode = "emergency"` against an ACP, UI, or
  Pubsub target
- **THEN** MCP surfaces the relay's `validation_invalid_params` unchanged
