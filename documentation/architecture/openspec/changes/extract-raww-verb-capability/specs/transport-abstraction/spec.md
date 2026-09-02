## MODIFIED Requirements

### Requirement: Transport Interface Contract

The relay delivery subsystem SHALL dispatch all agent delivery operations
through two non-blocking write methods defined on the `Transport` trait:

- `mailw` — structured relay message write. The relay SHALL populate routing,
  attribution, message body, timestamp, choice-decider, and quiescence fields
  before calling the transport. **The relay invokes per envelope**, and a
  transport MAY coalesce consecutively received envelopes into one packing unit;
  it SHALL NOT wait for target readiness before submitting. The transport SHALL
  render any transport-specific representation internally and resolve each member
  with a terminal `SingleDeliveryOutcome` derived from that member's packing-unit
  evidence.

  Coalescing is permitted **because the partition is declared rather than
  inferred**. An earlier draft of this requirement forbade a transport from
  buffering or coalescing, on the grounds that a group formed inside a transport
  made its membership unknowable to the relay — which is what allowed one outcome
  to be reported for members with different fates. `PartitionSink` removed that
  premise: a coalescing transport declares its unit's exact membership through the
  relay before any target-side effect, so the group is recorded even though it is
  timing-derived. The prohibition was guarding a hazard the declaration mechanism
  now excludes, and a rule that forbids what all three coder transports do is a
  rule that is not being enforced.
- `raww(content: String, append_enter: bool)` — raw input write. The `raww`
  capability's `Relay raww operation contract` governs its request shape.

**The invocation is fallible.** The relay's admission quota reserves count and
bytes in the relay's own queue and nothing about a transport's channel, its live
worker generation, UI subscriber capacity, or any target resource. A transport
SHALL be permitted to refuse an invocation, and a refusal SHALL be treated as a
terminal evidence result rather than as a reclaim:

- the transport returns the envelope **unchanged**, before partition → every
  member it would have covered resolves `not_submitted`;
- side effects **cannot be excluded** → the affected unit's members resolve
  `submission_unknown`.

The relay SHALL NOT reclaim or retry in either case.

**A transport SHALL NOT wait.** Post-authorization execution SHALL NOT wait on
prompt readiness, target turn completion, target output, or an operator decision.
No authorized batch SHALL sit in a transport staging queue behind an in-flight
turn. A submission primitive that can block SHALL be supervised and
fenced/interruptible per the `Transport Generation Fencing and Termination
Authority` requirement.

Each transport type SHALL implement these methods in its own module. The relay
SHALL dispatch via a `TransportImpl` enum that delegates without dynamic
allocation, and SHALL submit uniformly for every target with no transport-type
routing fork in the delivery loop. `TransportImpl` has **five** variants — `Acp`,
`Tmux`, `Pty`, `Ui`, and `Pubsub` — and this contract applies to all of them.

`mailw` and `raww` SHALL be the relay's only delivery seam. The relay worker
SHALL NOT pre-render pane-envelope text before calling `mailw`; representation
rendering belongs to the receiving transport. The legacy synchronous methods —
`deliver`, `prepare_delivery`, and `raw_write` — and the types that existed
solely to serve them SHALL NOT be retained.

The trait methods SHALL be non-blocking at the relay boundary. On relay shutdown,
still-pending relay-owned members resolve `dropped_on_shutdown`; authorized
members resolve from evidence.

#### Scenario: ACP delivery via TransportImpl

- **WHEN** the relay authorizes a batch for an ACP target
- **THEN** it invokes `TransportImpl::Acp(t)` with each authorized envelope
- **AND** the ACP transport partitions what it holds, having coalesced
  consecutive invocations or not, renders pane-envelope text internally, and
  submits each unit as its own `session/prompt` request
- **AND** it does not park an invocation behind an in-flight turn

#### Scenario: Tmux delivery via TransportImpl

- **WHEN** the relay authorizes a batch for a Tmux target
- **THEN** it invokes `TransportImpl::Tmux(t)` with each authorized envelope
- **AND** the Tmux transport partitions what it holds, having coalesced
  consecutive invocations or not, into token-budget prompts and injects each
  separately
- **AND** it does not wait for pane quiescence, which the relay has already done

#### Scenario: UI delivery via TransportImpl

- **WHEN** the relay authorizes a batch for a `Ui` target
- **THEN** it invokes `TransportImpl::Ui(t)` with the same structured message
  data used for coder transports
- **AND** the UI transport emits the messages as relay stream events through its
  injected broadcaster closure
- **AND** no `Ui`/`Pubsub` delivery short-circuit appears in the dispatch path

#### Scenario: A transport refuses an invocation before partition

- **WHEN** a transport's write channel is full or closed, or its worker
  generation is dead
- **THEN** it returns the envelope unchanged without partitioning it
- **AND** every member it would have covered resolves `not_submitted`
- **AND** the relay does not return them to `Pending`

#### Scenario: Shutdown resolves pending members

- **WHEN** relay shutdown is requested
- **THEN** still-`Pending` relay-owned members resolve `dropped_on_shutdown`
- **AND** `Authorized` members resolve from evidence

#### Scenario: Startup never runs on an async runtime thread

- **WHEN** the relay invokes `Transport::startup` for any session type
- **THEN** it runs the call on a blocking thread rather than on a runtime worker
  thread, because `startup` is synchronous on the trait and every implementation
  of it is therefore permitted to block
- **AND** the relay SHALL NOT make that choice per session type, so that a
  transport acquiring a blocking startup step later inherits the guarantee
  rather than an assumption about what it used to do
- **AND** because such a call cannot be aborted, each transport's `startup`
  SHALL own the cleanup of anything it created, reaching its own conclusion even
  when the caller awaiting it has gone away
