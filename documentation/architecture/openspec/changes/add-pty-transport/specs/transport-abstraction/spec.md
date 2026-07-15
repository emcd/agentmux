## MODIFIED Requirements

### Requirement: Transport Module Boundaries

ACP-specific delivery code SHALL reside in `src/acp/`. Tmux-specific delivery
code SHALL reside in `src/tmux/`. Pty-specific delivery code SHALL reside in
`src/pty/`. UI stream-broadcast delivery code SHALL reside in its own transport
module (`UiTransport`), not in the relay delivery subsystem. The relay delivery
subsystem SHALL NOT contain transport-specific logic; all transport dispatch SHALL
go through `TransportImpl`. Specifically, the relay delivery subsystem SHALL NOT
contain:

- quiescence scheduling or pane-identifier propagation,
- batch-combining or prompt-packing logic,
- pane-envelope rendering for coder transports,
- per-transport `TargetConfiguration::Acp`/`Tmux`/`Pty`/`Ui`/`Pubsub` dispatch
  arms for delivery, nor a relay-internal UI delivery path.

Every target SHALL be transport-delivered: `Ui` and `Pubsub` are first-class
transports (`TransportImpl::Ui`, and `TransportImpl::Pubsub` forward-declared as
a stub like the prior `Pty` stub), so the relay worker submits `mailw`/`raww`
uniformly without a transport-deliverability capability flag. The only
target-type-dependent step is transport construction.

#### Scenario: ACP code in src/acp/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no ACP-specific types or functions are present

#### Scenario: Tmux code in src/tmux/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Tmux pane operations, quiescence scheduling, rendering, or session
  lifecycle primitives are present

#### Scenario: Pty code in src/pty/

- **WHEN** a developer reads `src/relay/delivery/`
- **THEN** no Pty transport operations, libghostty-vt state access, portable-pty
  I/O, or shared wedge/prime state machine logic are present

#### Scenario: UI target delivered through its transport, not a relay path

- **WHEN** the relay receives a delivery task for a `Ui` target
- **THEN** it dispatches `mailw` through `TransportImpl::Ui` uniformly, with no
  transport-type routing fork
- **AND** no `TargetConfiguration::Ui | Pubsub` delivery arm or UI delivery
  short-circuit appears in the dispatch path

### Requirement: Worker Readiness Interface

The relay SHALL expose worker readiness through a transport-agnostic interface,
not an ACP-specific one. Any worker-driven transport (ACP today, Pty) that
maintains a multi-state readiness lifecycle SHALL populate the same surface:

- a transport-neutral readiness enum `WorkerReadinessState` with variants
  `Initializing`, `Available`, `Busy`, `Recovering`, and `Unavailable`, carrying
  no ACP-specific naming;
- a per-target registry field (`AsyncWorkerEntry.readiness`) holding one
  `Option<WorkerReadinessState>` — keyed implicitly by the per-target worker key
  `(namespace, runtime_directory, target_session)`, NOT a per-entry
  transport-keyed map, because each target is served by exactly one transport;
- relay-internal mutator/reader `set_worker_readiness` / `get_worker_readiness`;
- an in-process observer `subscribe_worker_readiness` (with publisher
  `publish_worker_readiness`) that yields the current readiness and every
  subsequent transition, and that MAY be subscribed before the worker registers
  and continues to receive transitions after the worker unregisters;
- a public read `read_worker_readiness` returning `Option<&'static str>` with the
  values `initializing` / `available` / `busy` / `recovering` / `unavailable`.

The interface SHALL NOT spell any of these symbols with an `acp`/`Acp` prefix.
Transport-specific readiness *triggers* (e.g. ACP stdin-write and terminal
stopReason mechanics; Pty flush-group dispatch and child exit) SHALL remain in
the owning transport module (`src/acp` and `src/pty` respectively), which drives
the shared interface rather than defining its own readiness type. Tmux does not
drive a multi-state worker readiness lifecycle and is therefore not a populator
of this interface.

#### Scenario: ACP worker populates the shared readiness interface

- **WHEN** the ACP worker transitions readiness (e.g. to `busy` on prompt-write
  success)
- **THEN** it calls `set_worker_readiness` with a `WorkerReadinessState` value
- **AND** in-process subscribers to `subscribe_worker_readiness` for that target
  observe the transition
- **AND** `read_worker_readiness` returns the corresponding state string

#### Scenario: Pty worker populates the shared readiness interface

- **WHEN** the Pty worker transitions readiness (e.g. to `busy` while a flush
  group is in flight, to `unavailable` on a `pane_wedged` outcome)
- **THEN** it calls `set_worker_readiness` with a `WorkerReadinessState` value
- **AND** in-process subscribers to `subscribe_worker_readiness` for that target
  observe the transition
- **AND** `read_worker_readiness` returns the corresponding state string

#### Scenario: Readiness surface carries no ACP-specific naming

- **WHEN** a developer reads the worker readiness enum, registry field, observer,
  and public read
- **THEN** none is spelled with an `acp`/`Acp` prefix
- **AND** a second worker-driven transport can populate the same surface without
  introducing a parallel readiness type

#### Scenario: Subscription survives the worker registration window

- **WHEN** a caller subscribes via `subscribe_worker_readiness` before the worker
  for that target is registered
- **THEN** the subscription is established against the per-target publisher
- **AND** the caller observes transitions published once the worker exists and
  after it later unregisters

## ADDED Requirements

### Requirement: Pty Transport Implementation

The system SHALL provide a `PtyTransport` in `src/pty/transport.rs` that
implements the existing `Transport` trait and is wired into
`TransportImpl::Pty(PtyTransport)`. The transport SHALL own one
`libghostty_vt::Terminal<'static, 'static>`, one `portable_pty` master, one
reader thread, and one delivery task. Because all `libghostty_vt` types are
`!Send + !Sync`, the terminal SHALL live on the delivery thread and be reached
from other threads (the relay worker for `mailw`/`raww` dispatch is
non-blocking — the transport enqueues onto an internal `mpsc::Sender`; the look
path is the only direct cross-thread accessor) through an `Arc<Mutex<PtyState>>`
shared handle.

#### Scenario: Pty startup spawns the child PTY and installs effect handlers

- **WHEN** the relay calls `TransportImpl::Pty(t).startup(context)` for a
  Pty-backed bundle member
- **THEN** the transport opens a `portable_pty` master sized to the per-coder
  `cols` and `rows`
- **AND** spawns the configured child command with `COLORTERM=truecolor` and
  a `TERM` env-var value derived from the per-coder `term-protocol` field
  (defaulting to `xterm-256color` when `term-protocol` is unset; see
  `pty-terminal-protocols` for the configurable `term-protocol` surface)
- **AND** constructs a `libghostty_vt::Terminal` with the same dimensions and
  installs the canonical effect handlers (`on_pty_write`, `on_size`,
  `on_device_attributes`, `on_xtversion`, `on_title_changed`)
- **AND** spawns the reader thread and the delivery task
- **AND** publishes `WorkerReadinessState::Available` via
  `set_worker_readiness`
- **AND** returns `TransportStatus { readiness: Ready }`

#### Scenario: Pty mailw enqueues and resolves via delivery task

- **WHEN** the relay calls `TransportImpl::Pty(t).mailw(envelope)` for a
  Pty-backed target
- **THEN** the transport enqueues the envelope on its internal `mpsc::Sender`
  and returns an `OutcomeFuture` immediately
- **AND** the delivery task picks up the envelope, renders the pane-envelope
  text via `DeliveryMessage::render_pane_envelope`, writes the bytes to the
  PTY master, drains `on_pty_write` responses back to the master, waits for
  quiescence via the shared wedge/prime state machine, and resolves the
  outcome future with the corresponding `SingleDeliveryOutcome`

#### Scenario: Pty look returns formatter text + cursor via shared handle

- **WHEN** the relay calls `OutputView::look` on the `PtyOutputView` returned
  by `give_output`
- **THEN** the look implementation locks the shared `PtyState` mutex, recreates
  the formatter from `&terminal`, calls `format_alloc(Format::Plain)`, splits
  the result on `\n`, takes the last `mode.lines.unwrap_or(40)` rows, and
  reads the cursor position via `terminal.cursor_x()` / `cursor_y()`
- **AND** returns `LookSnapshotPayload::Lines { snapshot_lines }`

#### Scenario: Pty shutdown closes PTY master and reaps child

- **WHEN** the relay calls `TransportImpl::Pty(t).shutdown()`
- **THEN** the transport publishes `WorkerReadinessState::Unavailable`
- **AND** closes the PTY master (causing the reader thread to exit)
- **AND** sends SIGTERM to the child, waits a short grace period, then
  SIGKILL if the child has not exited
- **AND** reaps the child via `child.wait()`
- **AND** drops the shared `PtyState`, releasing the terminal and render state

### Requirement: Generalized Wedge/Prime State Machine

The system SHALL provide a transport-agnostic wedge detection and prime
timeout state machine in `src/transports/quiescence.rs`, shared by all
promptable transports (Tmux, Pty). The state machine SHALL operate over
a `WedgeProbe` trait that exposes a single-snapshot observation shape:

- `observe(&mut self) -> Result<WedgeObservation, String>` — captures
  the probe's current state as a single snapshot. The state machine
  calls this twice per quiescence iteration (before and after the
  `wait_for_change` round). Implementations read any underlying IPC /
  state once and return a consistent snapshot.
- `wait_for_change(&mut self, deadline: Instant) -> Result<(), DeliveryWaitError>`
  — blocks until the next `observe()` call would differ from the
  previous one, or the supplied `deadline` elapses. Returns `Ok(())`
  on observed change; `Err(DeliveryWaitError::Timeout)` on deadline
  elapsed with no change; `Err(DeliveryWaitError::Failed)` on probe
  errors. The state machine passes a `deadline` derived from the
  per-coder `prime_timeout_ms` so the probe honors the same prime
  window the loop tracks.

The single-snapshot shape is intentional: a multi-method trait
would do 4-8x more work per iteration when the probe side-effects
on each call, and the existing probe test fixtures'
`abort_after_calls` counters would trip prematurely. The 16-probe
test surface in `tests/unit/tmux_transport.rs` uses this two-method
shape and preserves its `next_evaluation` cadence.

The `WedgeObservation` snapshot SHALL carry these fields (consistent
across all transports; per-transport probes populate them from their
native primitives):

- `inspected_tail: String` — the last `inspect_lines` rows formatted
  for prompt-readiness matching. Empty / whitespace-only indicates
  an empty pane (Unresponsive territory); non-empty + not
  prompt-ready indicates a wedge-class mismatch (Wedged territory).
- `is_prompt_ready: bool` — whether the target is currently
  prompt-ready. The state machine's `running` branch returns `Ok`
  when this is `true`.
- `pane_target: Option<String>` — active pane id (e.g. Tmux `%0`)
  for diagnostic inscriptions. `None` when the probe does not
  surface a pane target (e.g. Pty, which has no tmux-style pane
  id); the state machine omits the field from diagnostics in that
  case.
- `mismatch: Option<ReadinessMismatch>` — readiness-mismatch
  metadata when `is_prompt_ready = false`. The state machine uses
  `mismatch.reason` for the wedge/prime-timeout `reason` payload,
  falling back to deriving a generic reason from the inspected tail
  when `None`.
- `activity_generation: u64` — terminal-output-write marker
  populated at observation time. Tmux probes read
  `#{window_activity}` parsed as a `u64` epoch-seconds value
  (falling back to `0` when the format is unavailable on the
  running tmux version). Pty probes read
  `last_change_atomic.load(Ordering::Acquire)` from `PtyShared`. An
  advance between two consecutive observations signals that
  bytes were written to the target during the `quiet_window`,
  triggering the `Busy` pre-classification (see the
  `Three-State Delivery Classifier` requirement).

The state machine SHALL return the existing
`DeliveryWaitError::{Timeout, Wedged}` variants declared in
`src/transports/contract.rs`. Tmux and Pty SHALL share the state
machine; the per-transport adapter is the only divergence. The
Tmux transport constructs a small `TmuxAsWedgeProbe` adapter that
maps the existing `PaneQuiescenceProbe` into the new generalized
trait, preserving the 16-probe test surface in
`tests/unit/tmux_transport.rs` unchanged. The Pty transport
implements `WedgeProbe` directly in
`src/pty/state.rs::{PtyQuiescenceProbe, WorkerTerminalProbe}`,
populating `WedgeObservation` fields from a shared `PtyShared`
handle.

#### Scenario: Generalized state machine classifies based on probe results

- **WHEN** the shared wedge/prime state machine observes a flush
  group whose probe reports `is_prompt_ready == false` (prompt-
  readiness template does not match the inspected tail)
- **AND** wedge detection is enabled (per-coder config)
- **THEN** the state machine returns `DeliveryWaitError::Wedged { reason }`
  after `WEDGE_CONSECUTIVE_TICKS` (3) identical wedge-class
  evaluations, OR when the prime window has elapsed with a
  wedge-class mismatch observed
- **AND** the calling transport maps the error to
  `SendOutcome::Failed` + `reason_code = "pane_wedged"`

#### Scenario: Tmux adapter maps PaneQuiescenceProbe into WedgeProbe::observe

- **WHEN** a Tmux-backed flush group's `TmuxAsWedgeProbe::observe`
  is called
- **THEN** the adapter invokes the underlying `PaneQuiescenceProbe`
  exactly once and packages the result into a `WedgeObservation`
  whose fields (`inspected_tail`, `is_prompt_ready`, `pane_target`,
  `mismatch`, `activity_generation`) reflect the live pane state at
  the moment of the call
- **AND** the Tmux-side wedge/prime semantics match the merged
  `tmux-wedge-detection` and `add-wedge-detection-busy-state`
  proposals unchanged

> **Re-scoped 2026-07-15 against the post-`remove-operator-
> interaction-delivery-gate` archive (master `2708884`).** The
> prior draft described a four-method trait shape
> (`inspect_tail` / `cursor_idle_at` / `is_settled` /
> `operator_interaction_active`) that was abandoned during
> implementation (per the deviation note recorded in
> `add-pty-transport/tasks.md` §2.2). The shipped two-method
> shape (`observe` / `wait_for_change`) returns a single
> `WedgeObservation` snapshot and is `!Send + !Sync`-safe. The
> `operator_interaction_active` field was retired by
> `remove-operator-interaction-delivery-gate` along with the
> upstream copy-mode gate (issues/relay/52).