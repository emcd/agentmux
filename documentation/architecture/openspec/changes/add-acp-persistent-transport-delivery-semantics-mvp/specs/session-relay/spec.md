## MODIFIED Requirements
### Requirement: ACP Transport Timeout Semantics

ACP-backed send operations SHALL use turn-wait timeout semantics rather than
pane-quiescence semantics.

For ACP targets:

- request-level `acp_turn_timeout_ms` SHALL apply as ACP turn-wait timeout
- coder-level `[coders.acp] turn-timeout-ms` SHALL provide default timeout
- if neither value is set, system default SHALL be `120000` ms
- precedence SHALL be:
  1. request `acp_turn_timeout_ms`
  2. coder `[coders.acp] turn-timeout-ms`
  3. system default `120000`

Transport-field validation SHALL be fail-fast:

- ACP target + `quiescence_timeout_ms` =>
  `validation_invalid_timeout_field_for_transport`
- tmux target + `acp_turn_timeout_ms` =>
  `validation_invalid_timeout_field_for_transport`
- request includes both timeout fields =>
  `validation_conflicting_timeout_fields`

#### Scenario: Apply request ACP timeout override

- **WHEN** a send request to ACP target includes `acp_turn_timeout_ms`
- **THEN** relay uses that value as ACP turn-wait timeout for that target

#### Scenario: Fall back to coder ACP timeout default

- **WHEN** a send request to ACP target omits `acp_turn_timeout_ms`
- **AND** coder defines `[coders.acp] turn-timeout-ms`
- **THEN** relay uses coder timeout value for that target

#### Scenario: Reject quiescence timeout for ACP target

- **WHEN** request targets ACP transport
- **AND** request includes `quiescence_timeout_ms`
- **THEN** relay returns `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject ACP timeout for tmux target

- **WHEN** request targets tmux transport
- **AND** request includes `acp_turn_timeout_ms`
- **THEN** relay returns `validation_invalid_timeout_field_for_transport`

#### Scenario: Reject conflicting timeout fields

- **WHEN** request includes `quiescence_timeout_ms` and `acp_turn_timeout_ms`
- **THEN** relay returns `validation_conflicting_timeout_fields`

## ADDED Requirements
### Requirement: ACP Sync Delivery Phase Contract

For `delivery_mode=sync` and ACP targets, relay SHALL use a two-phase contract.

Phase 1 (delivery acknowledgment):

- relay SHALL report target `outcome=delivered` when first ACP activity is
  observed (`session/update` notification or prompt result)
- phase-1 response SHALL include
  `details.delivery_phase = "accepted_in_progress"`

Phase 2 (terminal completion):

- terminal prompt completion SHALL drive relay-internal worker readiness state
- phase-2 completion SHALL NOT retroactively mutate phase-1 sync response
- phase-2 completion SHALL NOT be required sender-facing `send` output in MVP

#### Scenario: Return delivered on first ACP activity

- **WHEN** sync send targets ACP session
- **AND** relay observes first ACP activity before terminal completion
- **THEN** relay returns target `outcome=delivered`
- **AND** includes `details.delivery_phase = "accepted_in_progress"`

#### Scenario: Fail before first ACP activity

- **WHEN** sync send targets ACP session
- **AND** ACP transport fails before first activity is observed
- **THEN** relay returns terminal failure/timeout outcome for that target

### Requirement: ACP Terminal Readiness Tracking

Relay SHALL use ACP terminal completion signals to maintain internal worker
readiness state for scheduling.

MVP state model:

- `available`: worker healthy and ready for next prompt
- `busy`: prompt accepted and turn in progress
- `unavailable`: worker transport/process failure requiring restart

Transition contract:

- first ACP activity observed => `busy`
- terminal stopReason observed => `available`
- disconnect/error requiring restart => `unavailable`

MVP sender-surface contract:

- these transitions SHALL NOT require additional sender-facing `send` outputs
- send success semantics remain phase-1 delivery acknowledgment only

#### Scenario: Mark worker available on terminal stopReason

- **WHEN** ACP worker reports terminal stopReason for in-progress prompt
- **THEN** relay marks worker state as `available`
- **AND** subsequent sends MAY be admitted for that target

### Requirement: ACP Persistent Worker Lifecycle

Relay SHALL manage persistent ACP workers for ACP-backed sends.

Worker model SHALL be:

- one worker per target session
- serialized request queue per worker
- fixed MVP queue bound `max_pending = 64`

Backpressure contract:

- enqueue beyond bound SHALL fail with `runtime_acp_queue_full`

Disconnect/restart contract:

- disconnect before phase-1 acknowledgment =>
  `runtime_acp_connection_closed`
- disconnect after phase-1 acknowledgment SHALL keep response immutable and
  transition worker to `unavailable` for recovery

Restart sequence SHALL be:

1. spawn ACP process
2. initialize
3. select lifecycle (`session/load` when identity exists, else `session/new`)
4. prompt

Failure taxonomy SHALL include:

- `runtime_acp_initialize_failed`
- `runtime_acp_session_load_failed`
- `runtime_acp_session_new_failed`
- `runtime_acp_prompt_failed`
- `acp_turn_timeout`

#### Scenario: Reject enqueue beyond fixed queue bound

- **WHEN** ACP worker queue depth reaches `max_pending`
- **AND** relay receives another ACP send for same target
- **THEN** relay returns `runtime_acp_queue_full`

#### Scenario: Surface disconnect before phase-1 acknowledgment

- **WHEN** ACP worker disconnects before first activity is observed
- **THEN** relay reports `runtime_acp_connection_closed`

### Requirement: ACP Permission Request Policy Mapping

Relay SHALL handle ACP `session/request_permission` as part of ACP prompt
execution.

Authorization source-of-truth SHALL remain relay policy evaluation.
Adapters SHALL NOT apply shadow authorization.

Policy denial contract SHALL remain canonical:

- error code: `authorization_forbidden`
- details required minimum:
  - `capability`
  - `requester_session`
  - `bundle_name`
  - `reason`
- optional additive details MAY include:
  - `target_session` or `targets`
  - `policy_rule_id`
  - `permission_kind`
  - `request_id`

Permission infrastructure failure codes SHALL include:

- `runtime_acp_permission_timeout`
- `runtime_acp_permission_failed`

Sync mapping:

- denial before phase-1 acknowledgment => sync target `failed`
- denial after phase-1 acknowledgment => no response mutation; relay handles
  internal worker state/recovery as needed

#### Scenario: Deny permission with canonical authorization schema

- **WHEN** ACP permission request is denied by relay policy
- **THEN** relay returns `authorization_forbidden`
- **AND** denial details include canonical required fields

#### Scenario: Report permission timeout with runtime code

- **WHEN** ACP permission request does not resolve before permission timeout
- **THEN** relay returns `runtime_acp_permission_timeout`
