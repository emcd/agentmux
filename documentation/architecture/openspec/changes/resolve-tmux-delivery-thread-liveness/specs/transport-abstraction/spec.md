## MODIFIED Requirements

### Requirement: Synchronous Delivery Completion

`mailw()` and `raww()` SHALL each return an outcome future that resolves with a
terminal `SingleDeliveryOutcome` when the write reaches a terminal state; the
relay worker maps that outcome onto its `SendResult` (the future carries the
transport-side type, not the relay `SendResult`, preserving the no-relay-dependency
invariant). The relay worker performs sender fan-out by awaiting the returned
futures; there is no transport-issued completion callback or event separate from
the future. The transport SHALL NOT drop a write without resolving its outcome
future. On relay shutdown, all pending futures SHALL resolve with a
dropped/shutdown outcome promptly. This does not block the relay request path:
the send RPC returns `Queued` at enqueue, and outcome futures are awaited only
on the per-target worker.

A transport that owns a background delivery task SHALL observe that task's
liveness rather than infer it from the presence of a channel sender. For Tmux,
when the delivery thread has stopped, subsequent `mailw()` and `raww()` calls
SHALL resolve immediately with `SendOutcome::Failed` and
`reason_code = "tmux_delivery_thread_stopped"`; they SHALL NOT remain queued
behind a stale sender or leave their outcome futures pending. The existing
immediate terminal failure for a channel that races closed remains required.

#### Scenario: mailw future resolves on delivery

- **WHEN** the relay worker calls `mailw(envelope)` on a transport
- **THEN** it receives a future immediately
- **AND** the future resolves with a terminal `SingleDeliveryOutcome` once the
  transport delivers (or fails to deliver) the write, which the relay worker
  maps onto its `SendResult` at the collect site

#### Scenario: Shutdown resolves all pending futures

- **WHEN** relay shutdown is requested while outcome futures are pending
- **THEN** each transport resolves all pending futures with a dropped/shutdown
  `SingleDeliveryOutcome` promptly

#### Scenario: Tmux observes a stopped delivery thread

- **WHEN** a Tmux delivery thread has stopped after startup
- **AND** the transport still holds stale channel state
- **THEN** the transport observes the stopped thread before accepting a new
  write
- **AND** a subsequent `mailw()` or `raww()` future resolves immediately with
  `SendOutcome::Failed`
- **AND** the reason code is `tmux_delivery_thread_stopped`
- **AND** the write is not parked on the stale channel

#### Scenario: Tmux resolves a write when the channel closes during submission

- **WHEN** a Tmux delivery thread stops concurrently with a write submission
- **AND** the channel reports `Full` or `Closed` from `try_send`
- **THEN** the submitted write future resolves immediately with a terminal
  failure outcome
- **AND** the future does not remain pending
