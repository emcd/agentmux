## MODIFIED Requirements

### Requirement: Transport Interface Contract

The relay delivery subsystem SHALL NOT dispatch delivery by invoking a
transport method per envelope. Instead, each transport implementation of the
`Transport` trait in `src/transports/contract.rs` SHALL own one **serial
delivery-loop executor**, spawned during `startup` and living for the
transport instance's lifetime, which calls the relay's `peek` and `ack`
entry points (`delivery-quiescence`'s `Mailbox Peek Operation` and `Mailbox
Acknowledgment and Partial Acknowledgment` requirements) directly.

`mailw`, `raww`, and `is_ready_for_handover` are **removed from the `Transport`
trait**. The relay no longer invokes a transport to deliver mail, and no
longer reads a readiness level from it to gate anything. A transport MAY
still keep an internal readiness predicate — the `transport-contracts`
capability's prompt-readiness templates still exist and still govern
Tmux/Pty readiness determination — but that predicate now informs only the
transport's own choice of when to write what it peeked, not any relay-facing
call.

The delivery-loop executor's contract:

- it calls `peek(target, entry_max, canonical_bytes_max)` when notified by
  the delivery doorbell or when its bounded poll fires, per
  `delivery-quiescence`'s `Delivery Doorbell Notification` requirement;
- it renders what it peeked using its own transport-specific representation
  and measures the result against its own token budget, MAY coalesce
  consecutively peeked mail entries into one packing unit exactly as
  `mailw` invocations were previously coalesced, and writes a **prefix** of
  what it peeked — possibly all of it;
- it calls `ack(target, generation_id, through_seq, evidence)` for exactly
  the prefix it wrote, supplying per-member `SubmissionEvidence` derived
  from that write;
- it does not wait on prompt readiness, target turn completion, target
  output, or an operator decision before writing once it has decided to
  write; a submission primitive that can block SHALL be supervised and
  fenced/interruptible per the `Transport Generation Fencing and
  Termination Authority` requirement, unchanged by this proposal.

Coalescing remains permitted for the same reason it was permitted under the
push model: the partition is declared through `PartitionSink` before any
target-side effect, so the group is recorded even though its membership is
timing-derived.

**The write path remains fallible**, and a refusal remains a terminal
evidence result rather than a reclaim: a transport that cannot write what it
peeked simply does not `ack` it, leaving it `queued` for the next attempt,
which MAY be made by the same generation or a replacement. A transport MAY
also `ack` with `SubmissionUnknown` evidence for a unit whose side effects it
cannot exclude, exactly as it previously reported that evidence for a
push-model invocation.

Each transport type SHALL implement its delivery-loop executor in its own
module. `TransportImpl` retains its five variants — `Acp`, `Tmux`, `Pty`,
`Ui`, and `Pubsub` — and this contract applies to all of them.

The legacy synchronous methods — `deliver`, `prepare_delivery`, and
`raw_write` — remain not retained, as under the prior contract.

`raww` as a **relay-inbound** operation name is unaffected by this
requirement: a caller still invokes `raww` to submit raw input, and the
relay still admits it, but as a raw-kind mailbox entry rather than as a
direct push into a transport. `transport-contracts`' `Relay raww transport
behavior` requirement specifies how a transport's delivery-loop executor
discovers and writes it.

#### Scenario: ACP delivery via its own delivery-loop executor

- **WHEN** an ACP target's mailbox gains entries
- **THEN** the ACP transport's delivery-loop executor peeks them, having
  coalesced consecutive entries or not, renders pane-envelope text
  internally, submits each unit as its own `session/prompt` request, and
  acks what it wrote
- **AND** it does not park an unwritten peek behind an in-flight turn

#### Scenario: Tmux delivery via its own delivery-loop executor

- **WHEN** a Tmux target's mailbox gains entries
- **THEN** the Tmux transport's delivery-loop executor peeks them, having
  coalesced consecutive entries or not, into token-budget prompts, injects
  each separately, and acks what it wrote
- **AND** it does not wait for pane quiescence beyond its own readiness
  check before writing

#### Scenario: UI delivery via its own delivery-loop executor

- **WHEN** a `Ui` target's mailbox gains entries
- **THEN** the UI transport's delivery-loop executor peeks them and emits
  the messages as relay stream events through its injected broadcaster
  closure, then acks what it emitted
- **AND** no `Ui`/`Pubsub` delivery short-circuit appears in the mailbox
  path

#### Scenario: A refused write leaves its entries queued, not reclaimed

- **WHEN** a transport's delivery-loop executor peeks entries but its
  write channel is full, closed, or its executor is otherwise unable to
  write
- **THEN** it does not `ack` those entries
- **AND** they remain `queued` for the next `peek`, by the same generation
  or a replacement
- **AND** the relay does not treat the unacked peek as a refusal requiring
  its own terminal outcome

#### Scenario: Shutdown resolves unsubmitted members

- **WHEN** relay shutdown is requested
- **THEN** still-`queued` relay-owned members that no delivery-loop executor
  has begun writing resolve `dropped_on_shutdown`
- **AND** members whose write has begun resolve from evidence per the
  `delivery-quiescence` capability's guard

#### Scenario: Startup never runs on an async runtime thread

- **WHEN** the relay invokes `Transport::startup` for any session type
- **THEN** it runs the call on a blocking thread rather than on a runtime
  worker thread, because `startup` is synchronous on the trait and every
  implementation of it is therefore permitted to block, including spawning
  its own delivery-loop executor
- **AND** the relay SHALL NOT make that choice per session type
- **AND** because such a call cannot be aborted, each transport's `startup`
  SHALL own the cleanup of anything it created, reaching its own
  conclusion even when the caller awaiting it has gone away

### Requirement: Transport Handover Capacity and Readiness

The system SHALL recognize two relay-facing quantities, where three were
previously conflated:

| Quantity | Owner | Purpose |
|---|---|---|
| **Admission quota** | relay | how much may be queued per target and relay-global; enforced at admission |
| **Maximum peek dimensions** | transport, static | the most a `peek` call may return at once |

**Acceptance capacity is no longer a relay-facing quantity.** Under the push
model a transport surfaced "can I accept right now" as `is_ready_for_
handover`, which the relay read to decide whether to authorize. Under the
pull model there is nothing to authorize: a transport that cannot currently
write simply does not peek, or peeks and writes nothing. Whether a transport
is momentarily busy is therefore entirely its own internal state, consulted
by its own delivery-loop executor, and is not part of this contract.

All relay-facing quantities SHALL be expressed in units the relay can
evaluate without packing: **envelope count and canonical payload bytes**,
where canonical bytes means the serialized envelope payload the relay
already holds, not rendered target text. Declaring them in tokens would be
circular, since only the transport can render and count those.

**No transport → relay back-edge for readiness.** The relay does not read a
readiness level from a transport at all under this contract; readiness is
purely internal to the transport's own delivery-loop executor. Where a
transport needs to prompt the relay that its target's own state changed in a
way worth reporting (for example, to drive `look` freshness), it SHALL do so
through an opaque closure the relay provided at construction, unrelated to
mailbox consumption.

#### Scenario: Maximum peek dimensions are declared in relay-evaluable units

- **WHEN** a transport declares its maximum peek dimensions
- **THEN** they are expressed in envelope count and canonical payload bytes
- **AND** not in rendered tokens

#### Scenario: A busy transport is not a relay-visible state

- **WHEN** a transport's delivery-loop executor is not ready to write
- **THEN** it does not call `peek`, or calls `peek` and writes nothing
- **AND** the relay observes no readiness signal from the transport and
  makes no decision on the basis of one

## ADDED Requirements

### Requirement: Neutral Delivery Protocol Crate Boundary

The system SHALL hold the vocabulary shared by both delivery call
directions — relay-to-transport for `look`, transport-to-relay for
`peek`/`ack` — in a crate (or crate-internal module boundary) that both
sides depend on, promoted from `src/transports/vocabulary.rs` rather than
newly constructed.

This crate SHALL hold: mailbox entry and entry-kind representations, target
and consumer identity, consumer-generation binding, cursor position, the
`peek`/`ack` request and response shapes, and doorbell subscription
handles. It SHALL NOT hold `AsyncDeliveryTask`, `BundleMember`,
`TransportImpl`, or any `crate::relay` error type. `look`, startup, transport
generation fencing, concrete transport constructors, and `TransportImpl`
itself remain outside it.

**If either call direction still needs the other's concrete type to express
its contract, the inversion has not happened.** This requirement exists
specifically so the module-dependency direction the `Transport Module
Boundaries` requirement already forbids in one direction (transport
importing relay internals) does not silently re-appear in the other
direction as `peek`/`ack` are introduced (relay importing transport
internals to call them).

#### Scenario: The protocol crate compiles without relay or concrete-transport imports

- **WHEN** the neutral delivery protocol crate is built in isolation
- **THEN** it does not import `crate::relay`, `crate::acp`, `crate::tmux`,
  `crate::pty`, or `crate::transports::ui`
- **AND** it exposes the `peek`/`ack` request/response types and mailbox
  vocabulary needed by both call directions

#### Scenario: Neither call direction needs the other's domain type

- **WHEN** the relay's `look` handler and a transport's delivery-loop
  executor are each implemented against the neutral crate
- **THEN** neither imports a concrete type owned by the other side to
  express its own request or response shape
