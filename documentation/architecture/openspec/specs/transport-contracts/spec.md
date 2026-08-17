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

When `input_idle_cursor_column` is configured, the transport SHALL report itself
unable to accept a handover unless the cursor is at that configured column.

**The template gates authorization, and it gates it for every transport that has
one.** A target that is not prompt-ready SHALL NOT have a batch authorized for
it. This is a change in what the template does: it previously gated injection on
Tmux but on Pty only decided what the sender was told, because Pty had already
written the bytes.

**The template SHALL be evaluated by the transport that owns the target, never by
the relay.** The relay learns the result only as the level it reads through
`is_ready_for_handover`, and MUST NOT interpret `prompt_regex`, inspect pane
output, or compare a cursor column itself.

This is a decoupling boundary, not an implementation preference. Readiness
*determination* is transport-specific by nature and does not generalise: a prompt
regex over a pane tail is meaningless for ACP, whose readiness is the completion
of an earlier turn arriving on the wire protocol with no snapshot to inspect, and
meaningless again for UI, whose readiness is subscriber connectivity. A relay
that evaluated the template would be a relay that knows what a pane is, which the
`transport-abstraction` capability's `Transport Module Boundaries` requirement
forbids.

Readiness *scheduling* — deciding which target to visit, in what order, and when
to authorize — remains relay-owned and is transport-agnostic. The two are
separate concerns and only the second belongs to the relay.

A readiness failure SHALL be distinguished by its cause. A **frame mismatch**
(`prompt_regex` did not match the inspected tail) means the target has settled on
content that is not its prompt. A **cursor mismatch** (`prompt_regex` matched but
the reported cursor is not at `input_idle_cursor_column`) means the prompt frame
is healthy and the operator has input pending.

**Both mean the same thing operationally on every transport — do not authorize
yet — and neither SHALL be treated as evidence that the target has failed.** A
target that is not prompt-ready is a target that is not ready *now*; the reason
is not knowable from the inspected tail. A permission dialog awaiting an
operator, a compose box holding typed input, a coder producing no terminal output
while working, and a hung process all present as a settled non-prompt frame.

The per-transport split that previously let Pty conclude failure from a frame
mismatch is removed. No transport infers a terminal outcome from the template,
and the distinction between the two causes survives only as diagnostic
observability.

A target that never becomes prompt-ready **while its transport remains healthy**
SHALL leave its entry `Pending` indefinitely. No bound converts that wait into an
outcome, because how long a target stays busy is not evidence about the target.
The wait is reported through the `delivery-quiescence` capability's
undelivered-queue inscriptions.

This is bounded by health, not by time. A transport that reports itself
unreachable past the dwell threshold resolves its members per the
`transport-abstraction` capability's `Transport Health as a Separate Axis`
requirement — not because the wait grew long, but because sustained
unreachability is evidence that no wait will end it.

#### Scenario: Authorize when prompt-readiness template matches

- **WHEN** target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **THEN** relay authorizes the batch and the transport injects the message

#### Scenario: The relay never evaluates the template itself

- **WHEN** a target member has a prompt-readiness template
- **THEN** the relay delivery subsystem does not compile `prompt_regex`, read
  pane output, or compare a cursor column
- **AND** it authorizes solely on the level the transport reports through
  `is_ready_for_handover`

#### Scenario: A transport with no pane has no template to evaluate

- **WHEN** the target's transport observes readiness from a wire protocol or
  subscriber connectivity rather than pane output
- **THEN** it reports `is_ready_for_handover` from that observation
- **AND** the relay authorizes on the same level it reads for every other
  transport, with no transport-specific branch

#### Scenario: A frame mismatch is not a failure on any transport

- **WHEN** a target's output is quiescent and `prompt_regex` does not match
- **THEN** no terminal outcome is issued on that basis
- **AND** the entry remains `Pending`
- **AND** this holds identically for Tmux, Pty, and every other transport

#### Scenario: A cursor mismatch defers authorization

- **WHEN** `prompt_regex` matches but the reported cursor is not at
  `input_idle_cursor_column`
- **THEN** relay does not authorize a batch for that target
- **AND** issues no terminal outcome

#### Scenario: Match prompt plus status with one multiline regex

- **WHEN** target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **THEN** the transport reports itself able to accept a handover
- **AND** relay authorizes a batch for that target

#### Scenario: Require idle input column before injection

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position equals configured
  `input_idle_cursor_column`
- **THEN** relay authorizes the batch and the message is injected

#### Scenario: Do not inject while user is typing

- **WHEN** target member prompt-readiness template defines
  `input_idle_cursor_column`
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches inspected pane tail text
- **AND** the transport-reported cursor position differs from configured
  `input_idle_cursor_column`
- **THEN** relay does not authorize a batch for that target
- **AND** relay continues waiting until the target becomes prompt-ready or relay
  shuts down
- **AND** no terminal *failure* is issued on account of the pending input

#### Scenario: Do not inject into a pane awaiting an operator decision

- **WHEN** a target is displaying a prompt that awaits an operator response,
  such as a tool-permission request
- **AND** the pane is quiescent and `prompt_regex` does not match
- **THEN** relay does not authorize a batch for that target
- **AND** relay does not report a terminal failure on account of the settled
  non-prompt frame
- **AND** the message is delivered once the operator answers and the pane returns
  to its prompt, however long the operator takes
- **BECAUSE** a pane blocked on a human decision is neither ready nor failed, and
  the inspected tail cannot distinguish it from one that is

#### Scenario: Deliver to a pane the operator has scrolled into copy-mode

- **WHEN** the target pane is in tmux copy-mode (for example, the
  operator scrolled it with the mouse wheel)
- **AND** the pane's live content is prompt-ready
- **THEN** relay injects the message
- **AND** the pane remains in copy-mode with the operator's scroll
  position undisturbed

#### Scenario: A never-ready target waits without resolving

- **WHEN** a target's prompt-readiness template never matches, for arbitrarily
  long, while its transport keeps reporting itself reachable
- **THEN** the entry remains `Pending` and no terminal outcome is issued for it
- **AND** the most recent observation is recorded as diagnostics only, and does
  not accumulate toward any verdict

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

#### Scenario: Map successful ACP terminal stop reasons to delivered

- **WHEN** ACP prompt turn completes with terminal stop reason `end_turn`
- **THEN** relay reports target delivery outcome `delivered`
- **AND** sets `reason_code = null`

#### Scenario: Map cancelled to failed outcome

- **WHEN** ACP prompt turn completes with stop reason `cancelled`
- **THEN** relay reports target delivery outcome `failed`
- **AND** sets `reason_code = acp_stop_cancelled`

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
- pty target: write `text` to the PTY master; if `no_enter=false`, write the
  terminating newline after it
- ui target: emit `text` as a relay stream event through the transport's injected
  broadcaster closure

Relay SHALL treat raww `text` as opaque input and SHALL NOT evaluate shell
expansion or command substitution.

**Ordering.** Mail and raw are variants of one per-target relay FIFO.

Raw SHALL preserve FIFO: no authorization across a raw barrier, nor younger work
across older. It SHALL wait for **target-side ordering safety** of older mail,
which requires that execution has ceased — not merely that an outcome has become
terminal. A ledger transition to `submission_unknown` does not prove a
still-running submission cannot take effect later, so terminality is the weaker
condition and is not sufficient here.

Target-side ordering safety is established by the generation fence's positive
verdict, which is why the raw barrier is held until that verdict rather than
until the outcome resolves.

#### Scenario: Route raww to acp via session/prompt path

- **WHEN** raww target transport is `acp`
- **THEN** relay dispatches via existing shared ACP worker/client
  `session/prompt` path
- **AND** does not require a new ACP capability surface

#### Scenario: Default raww appends enter

- **WHEN** caller omits `no_enter`
- **THEN** relay treats `no_enter` as `false`
- **AND** appends Enter after injected text

#### Scenario: Raw preserves FIFO against pending mail

- **WHEN** a raww is submitted for a target that has older `Pending` mail
- **THEN** the older mail is authorized first
- **AND** the raw write follows it

#### Scenario: Raw waits for target-side ordering safety, not terminality

- **WHEN** a raww is submitted for a target with an authorized invocation already
  executing
- **AND** that invocation's members have resolved `submission_unknown`
- **THEN** the raw write still waits for the generation fence's positive verdict
- **AND** it does not proceed on the terminal outcome alone

#### Scenario: Terminal outcome does not release the raw barrier

- **WHEN** an older invocation's members resolve `submission_unknown`
- **AND** its submission execution has not been fenced
- **THEN** a waiting raw write does not proceed on the terminal outcome alone
- **BECAUSE** a terminal outcome resolves the member but does not prove the
  submission cannot still take effect

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

**The synchronous error code and the delivery outcome of the same spelling are
distinct and SHALL NOT be conflated.** The error code is returned at the request
boundary when a call cannot be accepted. The delivery outcome
`transport_unavailable` resolves an already-queued member and is governed by a
policy boundary:

- it SHALL fire only on a **positively observed terminal lifecycle state** — the
  transport was shut down, or its generation was torn down without replacement;
- a **transient absence** — a respawn in progress, a generation being replaced, a
  UI subscriber that has disconnected but whose session is still registered —
  SHALL leave members `Pending`, until the absence resolves into readiness, into
  a positively observed teardown, or into a sustained unreachability its
  transport reports as `Unreachable` past `[delivery].unreachable-dwell-ms`.
  Nothing converts the waiting itself into an outcome; what resolves the third
  case is the repeated observation, not its duration alone.

Otherwise `transport_unavailable` would become another inference from absence,
retired at the transport and reintroduced at the relay.

Selection between a terminal and a recovering lifecycle state SHALL be serialized
with queue scheduling, so a member cannot be resolved `transport_unavailable` by
one path while another is scheduling it against a live generation.

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

#### Scenario: A respawn in progress does not resolve transport_unavailable

- **WHEN** a target's transport generation is being replaced
- **AND** queued members are `Pending` for that target
- **THEN** those members remain `Pending`
- **AND** they resolve only if the replacement completes and they are authorized,
  or if the generation is instead torn down with no replacement

#### Scenario: A torn-down transport without replacement resolves its pending members

- **WHEN** a transport is shut down, or its generation is torn down with no
  replacement
- **THEN** its `Pending` members resolve `transport_unavailable`
- **AND** the outcome is issued from the positively observed lifecycle state, not
  from elapsed time

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
