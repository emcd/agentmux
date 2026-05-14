## ADDED Requirements

### Requirement: ACP Transport Error Code

Relay SHALL use dedicated error code `transport_unavailable` (in ACP context:
`acp_child_unavailable`) for failures caused by ACP child process write
failures or reader thread exit. This code SHALL be distinguishable from
`internal_unexpected_failure` which covers relay-internal logic errors.

Error code taxonomy addendum:

- `transport_unavailable` — ACP stdin write failure (child process dead or
  pipe broken); caller can retry by requesting a worker reconnect
- `internal_unexpected_failure` — relay-internal logic or lock failure;
  not a transport concern

#### Scenario: ACP stdin write failure returns transport_unavailable

- **WHEN** relay attempts to write a prompt to ACP stdin
- **AND** the write fails with an I/O error
- **THEN** relay returns error code `transport_unavailable`
- **AND** does not return `internal_unexpected_failure`

#### Scenario: transport_unavailable is distinguishable by MCP consumers

- **WHEN** MCP consumer receives an error response for an ACP send
- **AND** error code is `transport_unavailable`
- **THEN** consumer can infer the ACP process is gone and may retry/reattach
- **AND** can distinguish this from a non-retryable relay-internal failure

## MODIFIED Requirements

### Requirement: ACP Sync Delivery Phase Contract

For `delivery_mode=sync` and ACP targets, relay SHALL use a two-phase contract.

Phase 1 (delivery acknowledgment):

- relay SHALL report target `outcome=delivered` when the prompt write to ACP
  stdin succeeds (write-success equals delivery on a local stdio pipe)
- phase-1 response SHALL include
  `details.delivery_phase = "accepted_in_progress"`
- relay SHALL NOT wait for any `session/update` or other first-activity signal
  before returning phase-1

Wire-level phase tokens (`accepted_in_progress`, `accepted_dispatched`) are
unchanged.

Phase 2 (terminal completion):

- terminal prompt completion SHALL drive relay-internal worker readiness state
- phase-2 is signaled by the background reader when it receives the JSON-RPC
  response matching the prompt request-id
- phase-2 completion SHALL NOT retroactively mutate phase-1 sync response
- phase-2 completion SHALL NOT be required sender-facing `send` output in MVP

#### Scenario: Return delivered on prompt write success

- **WHEN** sync send targets ACP session
- **AND** relay successfully writes the prompt to ACP stdin
- **THEN** relay returns target `outcome=delivered`
- **AND** includes `details.delivery_phase = "accepted_in_progress"`
- **AND** does not wait for session/update or any other activity signal

#### Scenario: Fail on stdin write failure

- **WHEN** sync send targets ACP session
- **AND** ACP stdin write fails with an I/O error
- **THEN** relay returns terminal failure outcome with `transport_unavailable`
  error code for that target

### Requirement: ACP Terminal Readiness Tracking

Relay SHALL use ACP terminal completion signals from the background reader to
maintain internal worker readiness state for scheduling.

MVP state model:

- `available`: worker healthy and ready for next prompt
- `busy`: prompt accepted and turn in progress
- `unavailable`: worker transport/process failure requiring restart

Transition contract:

- successful prompt write to ACP stdin => `busy`
- background reader observes terminal `stopReason` in prompt response => `available`
- stdin write failure OR reader thread exit (any cause) => `unavailable`

The `busy` transition now occurs on write-success, not on first `session/update`
observation. Reader thread exit is an additional `unavailable` trigger,
mirroring the write-failure path.

MVP sender-surface contract:

- these transitions SHALL NOT require additional sender-facing `send` outputs
- send success semantics remain phase-1 delivery acknowledgment only

#### Scenario: Mark worker busy on prompt write success

- **WHEN** relay successfully writes a prompt to ACP stdin
- **THEN** relay marks worker state as `busy`

#### Scenario: Mark worker available on terminal stopReason

- **WHEN** ACP background reader receives JSON-RPC response to the prompt
  request-id with a terminal `stopReason`
- **THEN** relay marks worker state as `available`
- **AND** subsequent sends MAY be admitted for that target

#### Scenario: Mark worker unavailable on reader thread exit

- **WHEN** the ACP background reader thread exits (EOF, I/O error, or panic)
- **THEN** relay marks worker state as `unavailable`
- **AND** pending requests are drained with an error

### Requirement: ACP Persistent Worker Lifecycle

Relay SHALL manage persistent ACP workers for ACP-backed sends and ACP look
snapshot ingestion.

Worker model SHALL be:

- one worker per target session (one child process, one background reader thread)
- serialized request queue per worker
- fixed MVP queue bound `max_pending = 64`
- initialized during bundle startup/session startup pass for hosted bundles
- anchored by relay runtime context (relay socket/runtime directory), not tmux
  transport semantics
- never lazily created by ACP send/look request handlers

Worker startup sequence SHALL be:

1. spawn ACP child process
2. start background reader thread (owns child stdout)
3. initialize (register request-id, write to stdin, wait on oneshot)
4. select lifecycle (`session/load` when identity exists, else `session/new`)
5. worker transitions to `available` and accepts prompts

Worker shutdown sequence SHALL be:

1. close shared `Arc<Mutex<ChildStdin>>` (signal EOF to child)
2. drop child process handle
3. `join` background reader thread
4. release per-session state (replay buffer, pending-request registry)

Backpressure contract:

- enqueue beyond bound SHALL fail with `runtime_acp_queue_full`

Disconnect/restart contract:

- stdin write failure before phase-1 acknowledgment =>
  `runtime_acp_connection_closed`
- reader thread exit after phase-1 acknowledgment SHALL keep response
  immutable and transition worker to `unavailable` for recovery

Failure taxonomy SHALL include:

- `runtime_acp_initialize_failed`
- `runtime_acp_session_load_failed`
- `runtime_acp_session_new_failed`
- `runtime_acp_prompt_failed`
- `acp_turn_timeout`
- `runtime_acp_worker_unavailable`
- `transport_unavailable` (ACP child write failure or reader thread exit)

#### Scenario: Keep one authoritative worker for ACP send and look ingestion

- **WHEN** relay handles ACP send requests and ACP look reads for one target
- **THEN** lifecycle/reconnect ownership remains with one shared worker
- **AND** relay avoids dual ACP worker/client ownership for that target

#### Scenario: Start ACP worker during startup pass without lazy send/look bootstrap

- **WHEN** relay runs startup pass for a hosted bundle with ACP targets
- **THEN** relay initializes one ACP worker per configured ACP target
- **AND** ACP send/look request handlers do not lazily create ACP workers

#### Scenario: Return deterministic unavailable outcome when ACP worker is absent

- **WHEN** ACP send or ACP look is requested for a target whose ACP worker is
  unavailable
- **THEN** relay does not spawn a request-scoped ACP client
- **AND** send returns failure with `runtime_acp_worker_unavailable`
- **AND** look returns stale metadata with
  `stale_reason_code=acp_worker_unavailable`

#### Scenario: Worker teardown joins reader thread before releasing state

- **WHEN** an ACP worker is torn down (idle timeout, target removed, bundle stop)
- **THEN** relay closes child stdin, drops child process handle, and joins the
  reader thread before releasing per-session state
- **AND** no per-session state is accessed after join completes

### Requirement: ACP Look Freshness Derivation

Relay SHALL evaluate ACP look freshness deterministically from shared worker
state and update recency.

MVP deterministic thresholds:

- `acp_look_prime_timeout_ms = 750`
- `acp_stream_stalled_after_ms = 5000`

Authoritative age source precedence SHALL be:
1. `last_acp_frame_observed_at_ms` (updated by background reader on each
   `session/update` notification)
2. `last_snapshot_update_ms`
3. age unavailable (omit `snapshot_age_ms`)

Freshness predicate order SHALL be:
1. `worker_state=unavailable` => `freshness=stale`,
   `stale_reason_code=acp_worker_unavailable`.
2. snapshot empty:
   - prime timeout => `stale_reason_code=acp_snapshot_prime_timeout`
   - otherwise => `stale_reason_code=acp_worker_initializing`
3. snapshot non-empty:
   - `worker_state=busy` => `freshness=fresh` (MUST NOT emit
     `acp_stream_stalled`)
   - `worker_state=initializing` => `freshness=stale`,
     `stale_reason_code=acp_worker_initializing`
   - `worker_state=available` => `freshness=stale` with
     `stale_reason_code=acp_stream_stalled` only when stalled threshold is
     exceeded using authoritative age source precedence.

Freshness vocabulary semantics under continuous-reader model:

- `LiveBuffer` means "background reader thread is alive and feeding updates"
  (previously: "served from in-memory after request-scoped drain")
- `acp_stream_stalled` means "reader alive but observed N seconds of silence
  on the ACP stream" (previously: "drain window saw nothing")

Wire tokens (`Fresh`/`Stale`, `LiveBuffer`/`None`) are unchanged.

Relay SHALL treat machine freshness status as response-visible state for ACP
look.

Inscriptions/events MAY include the same freshness data as additive telemetry
but SHALL NOT be the sole machine carrier.

#### Scenario: Mark ACP look stale when worker is unavailable

- **WHEN** ACP look is served and shared worker state is unavailable
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_worker_unavailable`

#### Scenario: Keep ACP look fresh while worker is busy and snapshot exists

- **WHEN** ACP look is served with non-empty snapshot entries
- **AND** shared worker state is busy
- **THEN** relay returns `freshness=fresh`
- **AND** relay does not emit `stale_reason_code=acp_stream_stalled`

#### Scenario: Mark ACP look stale when available worker stream is stalled

- **WHEN** shared ACP worker is available
- **AND** no new ACP updates are observed for at least 5000ms using
  authoritative age-source precedence
- **THEN** relay returns success with `freshness=stale`
- **AND** `stale_reason_code=acp_stream_stalled`

#### Scenario: Background reader updates last_acp_frame_observed_at on each update

- **WHEN** background reader receives a `session/update` notification
- **THEN** `last_acp_frame_observed_at_ms` is updated to the current time
- **AND** freshness stalled threshold uses this timestamp as authoritative age

## REMOVED Requirements

### Requirement: ACP Transport Timeout Semantics (drain window clause)

**Reason**: The post-response drain window (`ACP_LOAD_POST_RESPONSE_DRAIN_TIMEOUT`,
`ACP_PROMPT_POST_RESPONSE_DRAIN_TIMEOUT`) and `drain_post_response_notifications`
are removed as dead code under the continuous-reader model. Turn-wait timeout
semantics for `session/prompt` are preserved; the drain window is not.

**Migration**: No migration needed. The background reader handles all stdout
reads continuously. Drain-timing-dependent integration tests are updated to
assert on background-reader behavior instead.

**Note**: The requirement text for ACP Transport Timeout Semantics is otherwise
preserved; only the drain window implementation detail is removed. The turn-wait
timeout contract (acp_turn_timeout_ms, coder turn-timeout-ms, system default
120000 ms) remains in force.
