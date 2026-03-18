## ADDED Requirements

### Requirement: ACP Send Lifecycle Selection Precedence

For ACP-backed send operations, runtime lifecycle selection SHALL use this
precedence order:

1. session config `coder-session-id` when present
2. relay-managed persisted ACP session id for that bundle session when present
3. otherwise `session/new`

This precedence supersedes coder-session-id-only lifecycle selection for ACP
send operations.

#### Scenario: Prefer configured coder-session-id for load

- **WHEN** target session is ACP-backed
- **AND** session config includes `coder-session-id`
- **THEN** relay selects ACP `session/load` using that configured id

#### Scenario: Use persisted session id when config id is absent

- **WHEN** target session is ACP-backed
- **AND** session config omits `coder-session-id`
- **AND** relay has a persisted ACP session id for that bundle session
- **THEN** relay selects ACP `session/load` using the persisted id

#### Scenario: Select session-new when no load identity exists

- **WHEN** target session is ACP-backed
- **AND** session config omits `coder-session-id`
- **AND** relay has no persisted ACP session id for that bundle session
- **THEN** relay selects ACP `session/new`

### Requirement: ACP Session Identity Persistence Ownership

Relay SHALL maintain durable ACP session-id state for ACP-backed bundle
sessions under runtime state ownership.

Relay SHALL update persisted ACP session-id state when ACP `session/new`
returns a new `sessionId`.

#### Scenario: Persist session id returned by session-new

- **WHEN** relay executes ACP `session/new` for an ACP-backed session
- **AND** ACP response includes `sessionId`
- **THEN** relay persists that `sessionId` for subsequent lifecycle selection

#### Scenario: Keep persisted state scoped to bundle session identity

- **WHEN** relay persists ACP session id state
- **THEN** the persisted value is associated with one bundle session identity
- **AND** is not reused across unrelated bundle sessions

### Requirement: ACP Load Path Fail-Fast Semantics

When ACP `session/load` is selected, load failure SHALL fail the target send
operation and SHALL NOT fall back to ACP `session/new` in the same operation.

#### Scenario: Fail send target on session-load failure

- **WHEN** relay selects ACP `session/load`
- **AND** the load operation fails
- **THEN** relay reports target send outcome as failed
- **AND** relay does not call ACP `session/new` for that target in the same
  send operation

### Requirement: ACP Capability Gating

Relay SHALL perform explicit ACP capability gating before lifecycle/prompt
execution.

Required gates:

- ACP `initialize` must succeed
- ACP `session/load` path requires advertised load-session capability
- ACP prompt path requires prompt-session capability

Capability-gating failures SHALL use canonical error taxonomy:

- ACP initialize failure SHALL return `runtime_acp_initialize_failed`
- missing ACP capability for load/prompt SHALL return
  `validation_missing_acp_capability`

For `validation_missing_acp_capability`, error details SHALL include:

- `target_session`
- `required_capability` (`session/load` | `session/prompt`)
- `reason`

#### Scenario: Reject load path when load capability is missing

- **WHEN** relay selects ACP `session/load`
- **AND** initialized ACP capabilities do not advertise load-session support
- **THEN** relay fails the target with `validation_missing_acp_capability`
- **AND** error details include
  `required_capability = "session/load"`

#### Scenario: Reject prompt path when prompt capability is missing

- **WHEN** relay attempts ACP prompt execution for target
- **AND** initialized ACP capabilities do not advertise prompt-session support
- **THEN** relay fails the target with `validation_missing_acp_capability`
- **AND** error details include
  `required_capability = "session/prompt"`

#### Scenario: Surface initialize failure with canonical runtime code

- **WHEN** relay cannot complete ACP initialize handshake
- **THEN** relay fails target processing with `runtime_acp_initialize_failed`

### Requirement: ACP Transport Timeout Semantics

ACP-backed send operations SHALL use turn-wait timeout semantics rather than
pane-quiescence semantics.

For ACP targets:

- request-level `quiescence_timeout_ms` SHALL be interpreted as ACP turn-wait
  timeout
- tmux pane-quiescence gates SHALL NOT be applied

#### Scenario: Apply request timeout as ACP turn-wait bound

- **WHEN** a send request to ACP target includes `quiescence_timeout_ms`
- **THEN** relay uses that value as ACP turn-wait timeout for that target

### Requirement: ACP Stop-Reason Outcome Mapping

Relay SHALL map ACP prompt terminal states into canonical send outcomes with
stable reason-code behavior.

Mapping SHALL include:

- ACP terminal stop reasons (`end_turn`, `max_tokens`, `max_turn_requests`,
  `refusal`) -> delivery outcome `delivered` with `reason_code = null`
- ACP terminal stop reason `cancelled` -> delivery outcome `failed` with
  `reason_code = acp_stop_cancelled`
- ACP dropped-on-shutdown behavior -> delivery outcome `failed` with
  `reason_code = dropped_on_shutdown`
- ACP turn timeout -> delivery outcome `timeout` with
  `reason_code = acp_turn_timeout`

#### Scenario: Map successful ACP terminal stop reasons to delivered

- **WHEN** ACP prompt turn completes with terminal stop reason `end_turn`
- **THEN** relay reports target delivery outcome `delivered`
- **AND** sets `reason_code = null`

#### Scenario: Map cancelled to failed outcome

- **WHEN** ACP prompt turn completes with stop reason `cancelled`
- **THEN** relay reports target delivery outcome `failed`
- **AND** sets `reason_code = acp_stop_cancelled`

#### Scenario: Map ACP turn timeout to timeout outcome

- **WHEN** ACP prompt turn does not complete before effective turn-wait timeout
- **THEN** relay reports target delivery outcome `timeout`
- **AND** sets `reason_code = acp_turn_timeout`
