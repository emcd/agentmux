# transport-contracts Specification

## Purpose

Per-transport execution contracts (tmux, ACP, raww, Pty): worker lifecycles, transport capability flags, prime/wedge timeouts, copy-mode-transparent injection, and inter-transport error codes.

## Requirements

### Requirement: Configurable tmux socket

The system SHALL derive the tmux socket path for all tmux operations from the
configured state root and bundle name.

#### Scenario: Derive socket from default runtime roots

- **WHEN** no runtime root overrides are provided
- **THEN** the system uses the bundle runtime socket path under the default
  state root

#### Scenario: Derive socket from explicit runtime state root

- **WHEN** an explicit runtime state root is configured
- **THEN** the system uses that derived bundle socket path for session checks, reconciliation,
  pane capture, and message injection

### Requirement: Prompt-Readiness Template Gating

The system SHALL support optional per-member prompt-readiness templates that
must match before relay injection.

A prompt-readiness template SHALL support:

- `prompt_regex` (required)
- `inspect_lines` (optional, defaults to a bounded tail window)
- `input_idle_cursor_column` (optional)

`prompt_regex` SHALL be evaluated against a multiline string built from the
inspected non-empty tail lines of pane output. The "pane output" source is
transport-specific: Tmux reads from `capture-pane`; Pty reads from
`Formatter::format_alloc(Format::Plain)` via the `PtyOutputView` look path.

When `input_idle_cursor_column` is configured, relay SHALL treat the target as
prompt-ready only when the transport reports the cursor at that configured
column. For Tmux, this is `tmux display-message -p`; for Pty, this is
`Terminal::cursor_x()`.

Wedge detection defaults to enabled for both Tmux-backed and Pty-
backed sessions (the operator MAY opt out per coder via
`[coders.<id>.{tmux,pty}].wedge-detection = false`). When wedge
detection is enabled and the pane settles at a non-prompt-ready
state, the coder transport SHALL classify the flush group as
`wedged` rather than waiting indefinitely. The wedge detection knob
is independent of the prompt-readiness template configuration.

The wedge classifier is the same `Wedged` outcome for both Tmux
and Pty: `SendOutcome::Failed` + `reason_code = "pane_wedged"`
after `WEDGE_CONSECUTIVE_TICKS` (3) identical wedge-class
evaluations, OR when the prime window has elapsed with a wedge-
class mismatch observed. Per-transport knobs and Pty-specific
wedge scenarios live under the `Pty Wedged State Detection`
requirement; per-transport knobs live under the cross-cutting
`Pty Prime Timeout` requirement.

> **Re-scoped 2026-07-15 against the post-`remove-operator-
> interaction-delivery-gate` archive (master `2708884`).** The
> prior draft included a per-transport "Operator-interaction
> semantics differ between transports" subsection + three
> `operator_interaction_active`-conditional scenarios (Tmux
> silence, Pty always-false, Pty-doesn't-consult). All three
> are obsolete after the upstream copy-mode gate was retired
> (issues/relay/52). Pty wedge scenarios moved to the `Pty
> Wedged State Detection` ADDED requirement.

#### Scenario: Deliver when prompt-readiness template matches

- **WHEN** target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **THEN** relay injects the message

#### Scenario: Match prompt plus status with one multiline regex

- **WHEN** target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **THEN** relay treats target as prompt-ready

#### Scenario: Require idle input column before injection

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position equals configured
  `input_idle_cursor_column`
- **THEN** relay injects the message

#### Scenario: Do not inject while user is typing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position differs from configured
  `input_idle_cursor_column`
- **THEN** relay does not inject the message
- **AND** relay continues waiting until wedge detection fires (when
  enabled), prime timeout fires (when enabled), or relay shuts down

#### Scenario: Time out when quiescent pane never becomes prompt-ready

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].prime-timeout-ms` is set to a
  finite millisecond value
- **AND** pane output never begins flowing within the prime window
- **THEN** the transport resolves the flush group as
  `SendOutcome::Timeout`
- **AND** relay does not inject the message

#### Scenario: Classify as wedged when settled pane is not prompt-ready (default-on)

- **WHEN** target member has a prompt-readiness template
- **AND** the coder defines `[coders.<id>.tmux]` or
  `[coders.<id>.pty]` with `wedge-detection` not disabled (it
  defaults to enabled)
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** the coder transport resolves the flush group as
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** relay does not inject the message

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the
  operator scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** relay injects the message
- **AND** the pane remains in copy-mode with the operator's scroll
  position undisturbed

#### Scenario: Wedge detection opt-out preserves prior behavior

- **WHEN** target member has a prompt-readiness template
- **AND** `[coders.<id>.{tmux,pty}].wedge-detection = false`
- **AND** pane output reaches quiescence
- **AND** template matching conditions are not true
- **THEN** relay continues waiting until the pane becomes
  prompt-ready, prime timeout fires (if enabled), or relay shuts
  down

### Requirement: Prompt-Readiness Template Validation

The system SHALL validate prompt-readiness template regex during bundle
configuration loading.

#### Scenario: Reject invalid prompt regex

- **WHEN** bundle configuration includes a malformed `prompt_regex`
- **THEN** bundle loading fails with a structured configuration validation
  error

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

Template placeholders SHALL be validated before reconciliation starts. Unknown
or unresolved placeholders SHALL fail configuration validation.

#### Scenario: Use resume command when coder-session-id is present

- **WHEN** a session includes `coder-session-id`
- **THEN** the system resolves startup command from coder `resume-command`
- **AND** substitutes `{coder-session-id}` with the session value

#### Scenario: Use initial command when coder-session-id is absent

- **WHEN** a session does not include `coder-session-id`
- **THEN** the system resolves startup command from coder `initial-command`

#### Scenario: Reject unresolved placeholder during validation

- **WHEN** a chosen command template requires placeholders not provided by the
  session definition
- **THEN** the system rejects configuration with a validation error

### Requirement: Coder-Scoped Prompt-Readiness Templates

The system SHALL allow prompt-readiness templates to be defined per coder.
Sessions that reference a coder inherit that coder's prompt-readiness settings.

#### Scenario: Apply prompt regex from referenced coder

- **WHEN** a session references a coder that defines `prompt-regex`
- **THEN** relay evaluates prompt readiness for that session using the coder
  template

#### Scenario: Use coder prompt inspect line setting when configured

- **WHEN** a coder defines `prompt-inspect-lines`
- **THEN** relay uses that value as the prompt-readiness inspection window for
  sessions that reference the coder

#### Scenario: Use coder prompt idle column when configured

- **WHEN** a coder defines `prompt-idle-column`
- **THEN** relay requires tmux `cursor_x` to match that value before injection
  for sessions that reference the coder

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

### Requirement: ACP Terminal Readiness Tracking

Relay SHALL use ACP terminal completion signals from the background reader to
maintain worker readiness state for scheduling. The readiness state ACP
maintains SHALL be the shared transport-neutral `WorkerReadinessState` recorded
in the relay's transport-agnostic worker-readiness registry (see the
transport-abstraction "Worker Readiness Interface" requirement); ACP is one
populator of that registry, not the owner of a private readiness type. The
transition triggers below remain ACP-specific.

State model:

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

Sender-surface contract:

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

#### Scenario: ACP populates the shared worker-readiness registry

- **WHEN** ACP records any readiness transition
- **THEN** it writes a `WorkerReadinessState` value into the transport-agnostic
  worker-readiness registry via `set_worker_readiness`
- **AND** observers of `subscribe_worker_readiness` / `read_worker_readiness` see
  the transition without any ACP-specific observer name

### Requirement: ACP Persistent Worker Lifecycle

Relay SHALL manage persistent ACP workers for ACP-backed sends and ACP look
snapshot ingestion.

Worker model SHALL be:

- one worker per target session (one child process, one background reader thread)
- serialized request queue per worker
- fixed queue bound `pending_max = 64`
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

### Requirement: ACP Permission Request Readiness Signal

Relay SHALL treat ACP `session/request_permission` as in-progress turn activity
for ACP readiness tracking.

Behavior contract:

- `session/request_permission` observed before terminal completion SHALL count
  as first activity for two-phase sync acknowledgment semantics
- worker readiness SHALL transition to `busy` while turn completion remains
  pending
- terminal stopReason completion SHALL transition readiness to `available`

#### Scenario: Treat permission request as first ACP activity

- **WHEN** relay observes ACP `session/request_permission` before prompt result
- **THEN** relay marks worker state `busy` for the duration of the permission turn
- **AND** worker returns to `available` only after terminal stopReason is observed

#### Scenario: Keep worker non-ready while permission turn is in progress

- **WHEN** ACP `session/request_permission` is observed mid-turn
- **THEN** relay marks worker state `busy`
- **AND** relay does not consider that worker ready for next delivery until
  terminal stopReason is observed

### Requirement: Relay raww operation contract

Relay SHALL expose a raw direct-write operation named `raww` for a single
explicit target session.

Request contract:
- `target_session` (required)
- `text` (required UTF-8 string)
- `no_enter` (optional boolean, default `false`)
- `request_id` (optional)
- optional bundle selector with same-bundle-only enforcement

`raww` SHALL NOT support broadcast.

#### Scenario: Reject raww broadcast shape

- **WHEN** caller attempts to invoke `raww` without one explicit
  `target_session`
- **THEN** relay rejects the request with `validation_invalid_params`

### Requirement: Relay raww transport behavior

Relay raww transport execution SHALL map as follows:
- tmux target: inject literal `text` into target pane; if `no_enter=false`,
  inject Enter after text
- acp target: submit `text` using existing shared ACP worker/client path via
  `session/prompt`

Relay SHALL treat raww `text` as opaque input and SHALL NOT evaluate shell
expansion or command substitution.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** raww target transport is `acp`
- **THEN** relay dispatches via existing shared ACP worker/client
  `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false`
- **AND** appends Enter after injected text

### Requirement: Relay raww response contract

Relay raww immediate success responses SHALL be queued-only: the response
confirms enqueue acceptance and SHALL NOT include terminal delivery outcome.
Terminal outcomes are reported out-of-band via `delivery_outcome` stream events.

Required success fields:
- `status` (value `queued`)
- `target_session`
- `transport`

Optional success fields:
- `request_id`
- `message_id`

Failure responses SHALL use canonical relay error payload shape (`code`,
`message`, optional `details`). Only enqueue-time failures (e.g. ACP worker
unavailable) surface synchronously.

#### Scenario: Return queued payload for raww dispatch

- **WHEN** raww request to any writable target is accepted at dispatch boundary
- **THEN** relay returns success with `status = "queued"`
- **AND** includes required fields `target_session` and `transport`

### Requirement: Relay raww input bounds

Relay raww SHALL accept UTF-8 multiline text and SHALL reject payloads
larger than 32 KiB (UTF-8 bytes) with `validation_invalid_params`.

#### Scenario: Reject oversized raww text payload

- **WHEN** raww `text` exceeds 32 KiB UTF-8 bytes
- **THEN** relay rejects with `validation_invalid_params`

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

### Requirement: Transport Capability Contract

Every target reachable via look or raww SHALL have four transport capabilities,
derived at check time from its unified registry entry's `SessionType` rather than
stored as fields on the entry:

- `can_be_looked` — the session can be targeted by `look` (its transport
  supports snapshot capture)
- `can_be_written` — the session can be targeted by `raww` (its transport
  supports raw input injection)
- `can_stream_output` — the session's transport natively produces live
  output chunks (ACP and Pty stream output natively; Tmux requires periodic
  polling)
- `can_give_choices` — the session's transport can surface choice requests
  (the transport produces ACP-style option arrays for operator/UI resolution).
  Describes choice *production*, not resolution authority — any session with
  sufficient `choose` policy scope may resolve choices regardless of its own
  `can_give_choices` value.

Capabilities SHALL be derived from the entry's `SessionType` (at check time).
Bundle entries derive the type from bundle configuration at startup/reconcile;
relay-wide entries derive it from `users.toml` at startup for declared principals
(registered offline) or at Hello for dynamically-created principals. This makes
the registry entry the operation-time source of truth for target capabilities
instead of reloading different configuration sources for bundle and relay-wide
targets.

| Transport | `can_be_looked` | `can_be_written` | `can_stream_output` | `can_give_choices` |
|-----------|----------------|-----------------|--------------------|--------------------|
| `Tmux`    | true           | true            | false              | false              |
| `Acp`     | true           | true            | true               | true               |
| `Pty`     | true           | true            | true               | false              |
| `Ui`      | false          | false           | false              | false              |
| `Pubsub`  | false          | false           | false              | false              |

The `Pty` row is normative: Pty is a populated transport (per
`add-pty-transport`) and a coder-backed session whose coder defines
`[coders.<id>.pty]` derives `SessionType::Pty` with this capability row.
Bundle entries with `[coders.<id>.pty]` participate in `look` and `raww`
operations under the same capability checks as Tmux-backed entries.

`can_stream_output` is advertised on registration; streaming look semantics that
consume it are deferred to a follow-on proposal.

When a look or raww operation resolves a target whose entry-derived capability for
that operation is false, relay SHALL return `validation_unsupported_operation`.
This check precedes authorization policy checks and applies uniformly to bundle
targets and relay-wide targets.

#### Scenario: Reject look against session with can_be_looked false

- **WHEN** a `look` request resolves to a target whose `SessionType` derives
  `can_be_looked = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Reject raww against session with can_be_written false

- **WHEN** a `raww` request resolves to a target whose `SessionType` derives
  `can_be_written = false`
- **THEN** relay returns `validation_unsupported_operation`
- **AND** relay does not evaluate authorization policy for that request

#### Scenario: Permit look against session with can_be_looked true

- **WHEN** a `look` request resolves to a target whose `SessionType` derives
  `can_be_looked = true` (Tmux, ACP, or Pty)
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: Permit raww against session with can_be_written true

- **WHEN** a `raww` request resolves to a target whose `SessionType` derives
  `can_be_written = true` (Tmux, ACP, or Pty)
- **THEN** relay proceeds to authorization policy evaluation

#### Scenario: ACP session advertises can_give_choices true

- **WHEN** an ACP-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = true`

#### Scenario: Tmux session advertises can_give_choices false

- **WHEN** a Tmux-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = false`

#### Scenario: Pty session advertises can_give_choices false

- **WHEN** a Pty-backed session registers with the relay
- **THEN** its entry's `SessionType` derives `can_give_choices = false`

### Requirement: ACP Prime Timeout

The system SHALL surface a config-surfaced prime timeout knob for
ACP-backed sessions, applied as the `prime-timeout-ms` TOML key
under the per-coder `[coders.<id>.acp]` table. The key name is
identical to the Tmux-side key `[coders.<id>.tmux].prime-timeout-ms`
so operator vocabulary is symmetric across transports; the table
itself namespaces the transport.

The knob SHALL bound the time the ACP transport's internal delivery
task waits, during the per-turn prompt completion wait for a flush
group, for the target to produce a terminal ACP response before
classifying the flush group as `unresponsive`. The knob is
**opt-in**: when absent or `None`, the ACP transport preserves
today's unbounded behavior.

The prime timeout SHALL be communicated from the relay to the ACP
transport through the generic
`DeliveryEnvelope.prime_timeout_ms: Option<u64>` field introduced by
the `tmux-wedge-detection` proposal. The relay populates this field
from `[coders.<id>.acp].prime-timeout-ms` at envelope construction
time, for ACP-backed sessions. The ACP transport consumes the field
to bound the per-turn wait; it does NOT introduce a
transport-prefixed envelope field on top of the generic one.

The prime timer SHALL start at the moment the ACP transport's
internal delivery task first enters the per-turn wait
(`wait_for_prompt_complete`). The prime timer SHALL NOT reset on
coalesce-during-wait when new envelopes are absorbed into the
flush group; absorbed envelopes inherit the head envelope's prime
timer anchor.

The prime timer SHALL NOT classify a flush group as `unresponsive`
while a `pending_choice_outcome` is in flight (an operator
decision is pending). The prime timer continues to wait without
firing until the choice resolves or the turn completes. This
matches the non-expiring choice pending lifecycle contract.

When the prime timer fires (no terminal `PromptCompletion` AND no
pending choice within the prime window), the ACP transport SHALL
resolve every sender in the flush group with `SendOutcome::Timeout`
and `reason_code = "acp_turn_timeout"`. The transport SHALL NOT
inject further messages into the wedge; the failure is terminal and
the relay records a `delivery_prime_timeout` inscription event
carrying `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`.
The per-target readiness SHALL be latched to `Unavailable` so the
worker's respawn-needed signal can re-bootstrap the runtime on the
same path used for `PromptCompletion::ConnectionClosed`.

The `acp_turn_timeout` reason code SHALL be reused; no new
`SendOutcome` variant is introduced. The mapping is consistent
with the `ACP Stop-Reason Outcome Mapping` requirement (which
defines `acp_turn_timeout` as the canonical ACP timeout reason
code).

The prime timeout SHALL be config-only in v1. The pre-existing
per-call override surfaces (`--acp-turn-timeout-ms` CLI flag and
`acp_turn_timeout_ms` MCP payload field) are RETIRED — `send`
carries no per-call timeout override field in v1 on either
transport. Operators configure the deadline via the per-coder
config key only. The retirement is symmetric with the
`tmux-wedge-detection` retirement of `--quiescence-timeout-ms` and
`quiescence_timeout_ms` for Tmux: v1 of both transports is fully
config-only.

#### Scenario: ACP prime timeout fires on unresponsive ACP target

- **WHEN** the bundle config sets
  `[coders.<id>.acp].prime-timeout-ms` to a finite millisecond
  value
- **AND** the ACP transport's internal delivery task first enters
  the per-turn prompt completion wait for a flush group
- **AND** the ACP target produces no terminal `PromptCompletion`
  before the prime window elapses
- **AND** no `pending_choice_outcome` is in flight
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout` with `reason_code = "acp_turn_timeout"`
- **AND** no further message is injected into the target's
  prompt
- **AND** a `delivery_prime_timeout` inscription is emitted with
  `target_session`, `timeout_ms`, and `prime_wait_elapsed_ms`

#### Scenario: ACP prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.acp].prime-timeout-ms` (or sets it to `None`)
- **THEN** the ACP transport does not classify any flush group
  as `unresponsive`
- **AND** the only terminal failure modes for a flush group are
  the existing `ACP Stop-Reason Outcome Mapping` outcomes and
  `DroppedOnShutdown`

#### Scenario: ACP prime timeout does not fire during pending choice

- **WHEN** the bundle config sets
  `[coders.<id>.acp].prime-timeout-ms` to a finite millisecond
  value
- **AND** the ACP target's agent raises a tool-call permission
  request mid-turn (the `pending_choice_outcome` slot is in
  flight)
- **AND** the prime window elapses without a terminal
  `PromptCompletion`
- **THEN** the ACP transport continues to wait
- **AND** does NOT classify the flush group as `unresponsive`
- **AND** the prime timer continues to count down without firing
  while the choice is pending
- **AND** once the choice resolves (`ChoiceMade::Chosen` or
  `ChoiceMade::Cancelled`), the prime timer resumes counting
  against the original anchor

#### Scenario: ACP prime timer does not reset on coalesce-during-wait

- **WHEN** the ACP transport's internal delivery task is
  mid-turn for a flush group
- **AND** a new envelope arrives and is absorbed into the flush
  group via coalesce-during-wait
- **THEN** the prime timer continues to count down against the
  original prime window anchor (set at first wait start)
- **AND** the absorbed envelope does NOT extend or restart the
  prime window

#### Scenario: ACP prime timeout uses the generic envelope field

- **WHEN** an ACP-backed session has
  `[coders.<id>.acp].prime-timeout-ms` set to a finite
  millisecond value
- **THEN** the relay populates
  `DeliveryEnvelope.prime_timeout_ms` with that value at
  envelope construction time
- **AND** the ACP transport reads `prime_timeout_ms` to bound
  the prime wait
- **AND** no transport-prefixed envelope field (e.g.
  `acp_prime_timeout_ms`) is introduced

#### Scenario: ACP prime timeout uses the renamed operator knob

- **WHEN** the bundle config sets
  `[coders.<id>.acp].prime-timeout-ms` to a finite millisecond
  value
- **THEN** the `AcpTargetConfiguration.prime_timeout_ms` field
  (renamed from `turn_timeout_ms`) is validated at configuration
  load
- **AND** the prime timeout becomes load-bearing for the target
- **AND** operators who had configured the legacy
  `turn-timeout-ms` key (a key that does not exist in v1) see a
  `deny_unknown_fields` error from the raw loader on next bundle
  load

### Requirement: Tmux Prime Timeout

The system SHALL surface a config-surfaced prime timeout knob for
Tmux-backed sessions, applied as the `prime-timeout-ms` TOML key under
the per-coder `[coders.<id>.tmux]` table (no `tmux-` prefix; the table
itself namespaces the key). The knob SHALL bound the time the Tmux
transport waits, during the quiescence wait for a flush group, for the
target to produce observable output before classifying the flush
group as `unresponsive`. The knob is **opt-in**: when absent or
`None`, the Tmux transport preserves today's unbounded behavior.

The prime timeout SHALL be communicated from the relay to the Tmux
transport through a generic `DeliveryEnvelope.prime_timeout_ms:
Option<u64>` field. The relay populates this field from
`[coders.<id>.tmux].prime-timeout-ms` at envelope construction time.
The field is generic across transports: the relay does not know which
transport will consume it; the ACP delivery-side timeout follow-up
will populate the same field for ACP sessions from a corresponding
`[coders.<id>.acp].prime-timeout-ms` key.

The prime timer SHALL start at the moment the Tmux transport's
internal delivery task begins the quiescence wait for a flush group.
The prime timer SHALL NOT reset on coalesce-during-wait when new
envelopes are absorbed into the flush group during the prime window.

No transport-observable operator rendering state (tmux copy-mode or a
non-`root` client key-table) SHALL suppress the prime timer. A quiescence
wait SHALL always progress toward one of its terminal classifications; the
prime timer SHALL NOT be held off indefinitely on the basis of a rendering
signal the relay cannot bound.

When the prime timer fires (no observable output within the prime
window), the Tmux transport SHALL
resolve every sender in the flush group with `SendOutcome::Timeout`.
The relay worker SHALL propagate that outcome to the MCP/CLI caller
as a distinct timeout result, not collapsed into `Failed`.

#### Scenario: Prime timeout fires on unresponsive target

- **WHEN** the bundle config sets `[coders.<id>.tmux].prime-timeout-ms`
  to a finite millisecond value
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** the target pane produces no observable output before the
  prime window elapses
- **THEN** every sender in the flush group receives
  `SendOutcome::Timeout`
- **AND** no message is injected into the pane

#### Scenario: Prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Tmux transport does not classify any flush group as
  `unresponsive`
- **AND** the only terminal failure modes for a flush group are
  `Failed` + `reason_code = "pane_wedged"` (when wedge detection is
  enabled, which is the default) and `Shutdown`

#### Scenario: Prime timer does not reset on coalesce-during-wait

- **WHEN** the Tmux transport's internal delivery task is
  mid-prime-window for a flush group
- **AND** a new envelope arrives and is absorbed into the flush group
  via coalesce-during-wait
- **THEN** the prime timer continues to count down against the
  original prime window anchor (set at first wait start)
- **AND** the absorbed envelope does NOT extend or restart the prime
  window

### Requirement: Tmux Wedged State Detection

The system SHALL surface a config-surfaced wedge detection knob for
Tmux-backed sessions, applied as the `wedge-detection` boolean TOML
key under the per-coder `[coders.<id>.tmux]` table. The knob SHALL
classify a settled, non-prompt-ready pane as `wedged`.

Wedge detection defaults to **enabled** (`true`) — the cost of a
silently-wedged pane (delivery queue growth, silent failure) is
higher than the cost of a false-positive wedge (operator restarts the
target, future deliveries proceed normally). Operators MAY opt out by
setting `[coders.<id>.tmux].wedge-detection = false`. The opt-out
preserves today's unbounded-wait behavior.

A wedge detection SHALL fire when wedge detection is enabled and the
Tmux transport observes, during the quiescence wait for a flush
group:

- the pane output has been quiescent for at least one quiet window
- the prompt-readiness template does NOT match the inspected pane tail

When wedge detection fires, the Tmux transport SHALL resolve every
sender in the flush group with `SendOutcome::Failed` and
`reason_code = "pane_wedged"`. The classification SHALL be sticky:
once the flush group is classified as wedged, the transport SHALL NOT
re-evaluate across coalesce iterations. Per-message wedge deadlines
within a flush group are out of scope.

#### Scenario: Wedge fires on settled non-prompt-ready pane (default-on)

- **WHEN** the bundle config does not set
  `[coders.<id>.tmux].wedge-detection` (or sets it to `true`)
- **AND** the Tmux transport's quiescence wait observes the pane
  becomes quiescent
- **AND** the prompt-readiness template does not match the inspected
  pane tail
- **THEN** every sender in the flush group receives
  `SendOutcome::Failed` with `reason_code = "pane_wedged"`
- **AND** no message is injected into the pane

#### Scenario: Wedge detection opt-out preserves unbounded behavior

- **WHEN** the bundle config sets
  `[coders.<id>.tmux].wedge-detection = false`
- **THEN** the Tmux transport continues to wait past quiescence until
  the pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are
  `Timeout` (if prime timeout is enabled and fires) and `Shutdown`

#### Scenario: Wedge is sticky across coalesce iterations

- **WHEN** the Tmux transport's quiescence wait classifies a flush
  group as `wedged`
- **AND** new envelopes are absorbed into the flush group via
  coalesce-during-wait before the wedge classification propagates
- **THEN** every sender in the enlarged flush group receives the same
  wedge outcome (`Failed` + `reason_code = "pane_wedged"`)
- **AND** the transport does NOT re-evaluate wedge state across
  coalesce iterations

### Requirement: Copy-Mode-Transparent Injection

The Tmux transport SHALL inject the message body — and, when the write
requests submission, the submit — through mechanisms that write directly to the
target pane's pty and therefore bypass the tmux copy-mode key table.

The message body SHALL be injected with `paste-buffer` using bracketed paste
(`-p`), so that multi-line message content does not submit at its first
embedded newline.

Whether a write requests submission is governed by the existing per-write
submit flag (the `inject_literal_text` `append_enter` parameter): a normal
message delivery requests submission, and `raww` with `no_enter=false` requests
submission, while `raww` with `no_enter=true` does NOT (see the `Relay raww
transport behavior` requirement — "if `no_enter=false`, inject Enter after
text"). When and only when submission is requested, the submit SHALL be injected
as a separate **unbracketed** `paste-buffer` carrying a carriage return, NOT as
`send-keys`. A synthesized key — including `send-keys -H` with a raw byte — is
routed through the pane's active key table and is intercepted when the pane is
in copy-mode, so it SHALL NOT be used on the injection path. A body-only write
(`no_enter=true`) SHALL NOT synthesize a submit carriage return; it injects the
bracketed body and nothing else.

Delivery SHALL NOT be gated, deferred, or suppressed on the basis of tmux
copy-mode or a non-`root` client key-table. Such states do not affect the
child's ability to receive input, do not affect what `capture-pane` and
`cursor_x` report, and SHALL NOT be treated as a delivery precondition.

#### Scenario: Body and submit both reach a pane in copy-mode

- **WHEN** the target pane is in tmux copy-mode
- **AND** relay injects a submit-requesting write (a normal message delivery, or
  `raww` with `no_enter=false`)
- **THEN** the child process receives the complete message body
- **AND** the child process receives the submit carriage return
- **AND** `#{pane_in_mode}` still reports `1` after injection

#### Scenario: Body-only write does not synthesize a submit

- **WHEN** relay injects a body-only write (`raww` with `no_enter=true`)
- **THEN** the child process receives the message body
- **AND** the transport does NOT inject a submit carriage return
- **AND** this holds whether or not the pane is in copy-mode

#### Scenario: Multi-line body does not submit early

- **WHEN** relay injects a message body containing embedded newlines
- **THEN** the body is delivered as bracketed paste
- **AND** the target treats the embedded newlines as literal content rather
  than as submit keystrokes

### Requirement: Pty Prime Timeout

The system SHALL surface a config-surfaced prime timeout knob for Pty-backed
sessions, applied as the `prime-timeout-ms` TOML key under the per-coder
`[coders.<id>.pty]` table (no `pty-` prefix; the table itself namespaces the
key). The knob SHALL bound the time the Pty transport waits, during the
quiescence wait for a flush group, for the target to produce observable output
before classifying the flush group as `unresponsive`. The knob is **opt-in**:
when absent or `None`, the Pty transport preserves the unbounded behavior
inherited from the shared wedge/prime state machine.

The Pty prime timeout SHALL be communicated from the relay to the Pty transport
through the same generic `DeliveryEnvelope.prime_timeout_ms: Option<u64>` field
introduced by `tmux-wedge-detection`. The relay populates this field from
`[coders.<id>.pty].prime-timeout-ms` at envelope construction time. The field
is generic across transports: the relay does not know which transport will
consume it; ACP's wedge-companion proposal populates the same field for ACP
sessions.

The prime timer semantics for Pty follow the merged `tmux-wedge-detection`
proposal:

- The prime timer SHALL start at the moment the Pty transport's internal
  delivery task begins the quiescence wait for a flush group.
- The prime timer SHALL NOT reset on coalesce-during-wait when new envelopes
  are absorbed into the flush group during the prime window.
- The prime timer SHALL fire when the prime window has elapsed with no
  observable output from the target, regardless of any rendering-state
  signal; Pty has no operator-interaction concept that suppresses
  classification (the upstream copy-mode gate was retired by
  `remove-operator-interaction-delivery-gate`, archived 2026-07-15).
  Copy-mode and other rendering states do not impede injection or
  affect what `cursor_x` and `capture-pane`-style probes report; the
  prime timer always measures observable output, not rendering state.

When the prime timer fires for a Pty target (no observable output within the
prime window), the Pty transport SHALL resolve every sender in the flush group
with `SendOutcome::Timeout`. The `Timeout` outcome SHALL remain a distinct
terminal outcome and SHALL NOT be collapsed into `Failed`. The accept-time
async response for the originating send remains `queued`; the terminal
`Timeout` resolution is recorded per `Async Delivery Observability` and is
not returned to the synchronous caller.

> **Spec-alignment note (2026-07-16):** the prior wording
> "The relay worker SHALL propagate that outcome to the MCP/CLI caller"
> was an un-implemented in-band caller-propagation clause that was
> mirrored from the Tmux Prime Timeout requirement; it is removed from
> the Pty Prime Timeout delta for symmetry. The
> `Timeout`-distinct-from-`Failed` invariant is preserved as the
> shipped Pty behavior; an async sender-receipt surface for `Timeout`
> (and `Wedged` / `dropped_on_shutdown`) is not in Pty's surface
> today.

#### Scenario: Pty prime timeout fires on unresponsive target

- **WHEN** the bundle config sets `[coders.<id>.pty].prime-timeout-ms` to a
  finite millisecond value
- **AND** the Pty transport's internal delivery task begins the quiescence
  wait for a flush group
- **AND** the target produces no observable output before the prime window
  elapses
- **THEN** every sender in the flush group receives `SendOutcome::Timeout`
- **AND** no message is injected into the PTY

#### Scenario: Pty prime timeout defaults preserve unbounded behavior

- **WHEN** the bundle config does not set
  `[coders.<id>.pty].prime-timeout-ms` (or sets it to `None`)
- **THEN** the Pty transport does not classify any flush group as
  `unresponsive`
- **AND** the only terminal failure modes for a flush group are `Failed` +
  `reason_code = "pane_wedged"` (when wedge detection is enabled, the
  default) and `Shutdown`

### Requirement: Pty Wedged State Detection

The system SHALL surface a config-surfaced wedge detection knob for Pty-backed
sessions, applied as the `wedge-detection` boolean TOML key under the per-coder
`[coders.<id>.pty]` table. The knob SHALL classify a settled, non-prompt-ready
pane as `wedged` via the shared wedge/prime state machine in
`src/transports/quiescence.rs`.

Wedge detection defaults to **enabled** (`true`) for Pty, matching the merged
`tmux-wedge-detection` rationale (cost of a silently-wedged pane is higher
than cost of a false-positive wedge). Operators MAY opt out by setting
`[coders.<id>.pty].wedge-detection = false`. The opt-out preserves the
unbounded-wait behavior.

A wedge detection SHALL fire when wedge detection is enabled and the Pty
transport observes, during the quiescence wait for a flush group:

- the pane output has been quiescent for at least one quiet window (probe
  `observe()` returns `is_prompt_ready = false` and the
  `activity_generation` field has not advanced since the previous
  observation)
- the prompt-readiness template does NOT match the inspected pane tail
  (formatter `format_alloc(Format::Plain)` tail text does not match
  `prompt_regex`)

When wedge detection fires, the Pty transport SHALL resolve every sender in
the flush group with `SendOutcome::Failed` and `reason_code = "pane_wedged"`.
The classification SHALL be sticky: once the flush group is classified as
wedged, the transport SHALL NOT re-evaluate across coalesce iterations.
Per-message wedge deadlines within a flush group are out of scope.

#### Scenario: Pty wedge fires on settled non-prompt-ready pane (default-on)

- **WHEN** the bundle config does not set
  `[coders.<id>.pty].wedge-detection` (or sets it to `true`)
- **AND** the Pty transport's quiescence wait observes the pane becomes
  quiescent
- **AND** the prompt-readiness template does not match the inspected pane
  tail (read via `Formatter::format_alloc(Format::Plain)`)
- **THEN** every sender in the flush group receives `SendOutcome::Failed`
  with `reason_code = "pane_wedged"`
- **AND** no message is injected into the PTY

#### Scenario: Pty wedge detection opt-out preserves unbounded behavior

- **WHEN** the bundle config sets `[coders.<id>.pty].wedge-detection = false`
- **THEN** the Pty transport continues to wait past quiescence until the
  pane becomes prompt-ready or the relay shuts down
- **AND** the only terminal failure modes for the flush group are `Timeout`
  (if prime timeout is enabled and fires) and `Shutdown`

#### Scenario: Pty wedge is sticky across coalesce iterations

- **WHEN** the Pty transport's quiescence wait classifies a flush group as
  `wedged`
- **AND** new envelopes are absorbed into the flush group via
  coalesce-during-wait before the wedge classification propagates
- **THEN** every sender in the enlarged flush group receives the same wedge
  outcome (`Failed` + `reason_code = "pane_wedged"`)
- **AND** the transport does NOT re-evaluate wedge state across coalesce
  iterations

### Requirement: Pty Default Per-Coder Dimensions

The system SHALL provide per-coder default grid dimensions for Pty-backed
sessions, applied as the `cols` and `rows` TOML keys under the per-coder
`[coders.<id>.pty]` table. Both keys default to:

- `cols = 120`
- `rows = 40`

The Pty transport SHALL spawn the child under a `portable_pty` master sized to
these dimensions and construct `libghostty_vt::Terminal::new(TerminalOptions
{ cols, rows, max_scrollback: 10_000 })` with the same dimensions. Runtime
resize (via a future `agentmux resize <session> <cols> <rows>` command) is
out of scope for `add-pty-transport` and deferred to a follow-up proposal.

`look()` SHALL return `LookSnapshotPayload::Lines { snapshot_lines }` from
`Formatter::format_alloc(Format::Plain)` truncated to the consumer's
`LookMode.lines`. The terminal's actual grid may be any size (post-resize or
post-reflow); the consumer asks for what it wants. There is no requirement
that the relay-tui consumer's viewport match the Pty-backed session's grid
dimensions; multi-viewer dimension reconciliation is out of scope for
`add-pty-transport` and deferred to a follow-up proposal.

> **Spec-alignment note (2026-07-16, Pty archive):** the prior wording
> "SHALL call `Terminal::resize(cols, rows, 0, 0)` once at startup" is
> removed; the shipped Pty transport constructs the terminal at the
> configured dimensions and never calls `Terminal::resize`. The
> `cols` / `rows` keys continue to drive the `portable_pty` master size
> + the initial `TerminalOptions`. A future proposal may add a runtime
> resize path if needed.

#### Scenario: Pty spawns at per-coder default dims

- **WHEN** the bundle config does not set `[coders.<id>.pty].cols` or `.rows`
  (or sets them to the default values)
- **THEN** the Pty transport spawns the child under a 120 x 40 PTY master
- **AND** constructs `libghostty_vt::Terminal` with `cols = 120, rows = 40`

#### Scenario: Pty honors explicit per-coder dims

- **WHEN** the bundle config sets `[coders.<id>.pty].cols = 200` and
  `.rows = 60`
- **THEN** the Pty transport spawns the child under a 200 x 60 PTY master
- **AND** constructs `libghostty_vt::Terminal` with `cols = 200, rows = 60`

#### Scenario: Pty rejects zero-dimension config

- **WHEN** the bundle config sets `[coders.<id>.pty].cols = 0` or `.rows = 0`
- **THEN** the validator rejects the configuration with a structured config
  error during load
