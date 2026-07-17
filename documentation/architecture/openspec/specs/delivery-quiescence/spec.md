# delivery-quiescence Specification

## Purpose

Send envelope, async queue lifecycle, terminal outcomes, ack semantics, and asynchronous terminal-outcome receipt.

## Requirements

### Requirement: JSON Send Envelope

The system SHALL inject messages as strict, pretty-printed JSON envelopes.

Each envelope SHALL include:

- `schema_version`
- `message_id` (globally unique identifier)
- `sender_session`
- `target_session` or broadcast marker
- `created_at`
- `body`

#### Scenario: Inject valid envelope

- **WHEN** a send request is accepted for delivery
- **THEN** the system renders a strict, pretty-printed JSON envelope
- **AND** injects the envelope into the target session via tmux

#### Scenario: Reject malformed envelope input fields

- **WHEN** required message fields are missing or invalid
- **THEN** the system rejects the request with a validation error

### Requirement: Quiescence-Gated Delivery

The system SHALL avoid injecting a message while target session output is
actively changing. Quiescence gating is transport-internal: each transport that
supports quiescence (Tmux today) SHALL wait for the target to become idle before
flushing its internal write buffer. The relay delivery worker SHALL NOT
orchestrate quiescence; it delivers writes via `mailw` and awaits outcome futures.

The relay SHALL communicate per-write quiescence bounds to the transport via
two `DeliveryEnvelope` fields:

- `quiet_window: Duration` — the quiet period before the transport
  declares the target ready to receive a flush group. Shared across all
  transports that perform quiescence waits.
- `prime_timeout_ms: Option<u64>` — generic prime-timeout bound that any
  prime-wait transport MAY consume. The relay populates this field from
  per-coder config (e.g. `[coders.<id>.tmux].prime-timeout-ms` for Tmux
  sessions; the ACP delivery-side timeout follow-up will populate the
  same field from `[coders.<id>.acp].prime-timeout-ms` for ACP sessions).

The Tmux transport's prime timeout bounds the prime window (no
observable output during the quiescence wait before the timeout). The
Tmux transport SHALL NOT use the prime timeout to bound the
post-quiescence prompt-readiness wait, which is governed by the wedge
detection requirement and the prompt-readiness template requirement.
The per-transport bound semantics are recorded in the relevant
transport spec.

#### Scenario: Deliver after quiescent window

- **WHEN** the target pane output remains unchanged for the configured quiet
  window
- **THEN** the transport flushes its write buffer and injects the pending messages

#### Scenario: Continue waiting without timeout in async mode

- **WHEN** pane output continues changing
- **THEN** the transport keeps buffered writes pending
- **AND** flushes after a future quiescent window is observed

#### Scenario: Apply request prime timeout override on Tmux

- **WHEN** a Tmux-bound request carries a non-`None`
  `DeliveryEnvelope.prime_timeout_ms`
- **AND** the Tmux transport's internal delivery task begins the
  quiescence wait for a flush group
- **AND** no observable output is produced before that timeout
- **THEN** the Tmux transport resolves the pending outcome futures
  with `SendOutcome::Timeout`
- **AND** records a `delivery_prime_timeout` inscription in relay
  diagnostics

#### Scenario: Tmux prime timeout does not bound post-quiescence wait (wedge enabled)

- **WHEN** the target pane output becomes quiescent
- **AND** the prompt-readiness template does not match
- **AND** wedge detection is enabled (the default for the coder)
- **THEN** the Tmux transport SHALL NOT classify the flush group as
  `Timeout` solely on the basis of `prime_timeout_ms` elapsing
- **AND** the transport SHALL classify the flush group as `Failed`
  with `reason_code = "pane_wedged"` when the wedge detection
  requirement fires (after `WEDGE_CONSECUTIVE_TICKS` identical
  wedge-class evaluations or when the prime window has elapsed with
  a wedge-class mismatch observed)

#### Scenario: Tmux prime timeout bounds post-quiescence wait when wedge is disabled

- **WHEN** the target pane output becomes quiescent
- **AND** the prompt-readiness template does not match
- **AND** wedge detection is disabled via
  `[coders.<id>.tmux].wedge-detection = false`
- **AND** `prime_timeout_ms` is set to a finite millisecond value
- **THEN** the Tmux transport SHALL classify the flush group as
  `Timeout` when `prime_timeout_ms` elapses
- **BECAUSE** an operator who has explicitly disabled wedge detection
  and opted in to a prime timeout has accepted the bounded-wait
  semantics — the prime window is the only bounded-wait knob in
  effect, and it covers every quiescent state (including wedge-class
  content)

#### Scenario: Map Tmux prime timeout to transport envelope field

- **WHEN** a bundle member's `[coders.<id>.tmux].prime-timeout-ms` is
  set to a finite millisecond value
- **THEN** the relay attaches that value to the
  `DeliveryEnvelope.prime_timeout_ms` field at envelope construction
  time
- **AND** the Tmux transport uses it as the effective prime-window
  bound for the flush group

#### Scenario: Quiescence hints from head envelope govern the flush group

- **WHEN** the Tmux transport accumulates multiple envelopes with
  differing `quiet_window` or `prime_timeout_ms` values into one
  flush group
- **THEN** it uses the `quiet_window` and `prime_timeout_ms` from
  the first (head) envelope of the group as the effective bounds for
  the entire group
- **AND** a later envelope's prime timeout does not extend or
  shorten a wait already in progress for the group

### Requirement: Quiescence Documentation

The system SHALL document quiescence constraints and known interference
patterns for users configuring agent sessions.

#### Scenario: Document dynamic output caveat

- **WHEN** project documentation is generated for the relay capability
- **THEN** it includes a warning that continuously changing output sources
  (for example clock-style statusline content) can prevent quiescence
  detection from succeeding

### Requirement: Delivery Results Without ACK Protocol

Relay SHALL use asynchronous acceptance responses and SHALL NOT support
synchronous completion responses.

An accepted send request SHALL return immediately with per-target `outcome =
queued`. Relay SHALL NOT block the caller waiting for delivery completion.

#### Scenario: Report accepted async delivery

- **WHEN** relay accepts a send request for one or more targets
- **THEN** the immediate result marks those targets as `queued`
- **AND** does not wait for final delivery outcome before responding

#### Scenario: Return no-op completion for zero effective targets

- **WHEN** sender exclusion and target resolution produce zero effective
  recipients
- **THEN** relay returns an immediate no-op response without validation error
- **AND** response contains zero per-target results

### Requirement: Async Queue Lifecycle and Ordering

For `delivery_mode=async`, relay SHALL maintain an in-memory pending queue.
The queue SHALL be non-durable.
Relay SHALL preserve FIFO ordering per target session and SHALL NOT deduplicate
or coalesce queued messages.

#### Scenario: Drop pending async queue on relay restart

- **WHEN** relay exits or restarts before delivering queued async targets
- **THEN** pending async entries are discarded
- **AND** they are not recovered from durable storage

#### Scenario: Preserve per-target FIFO ordering

- **WHEN** multiple async messages are queued for the same target session
- **THEN** relay attempts delivery in enqueue order for that target

#### Scenario: Do not deduplicate queued async messages

- **WHEN** queued async messages have identical body content or same target set
- **THEN** relay treats them as distinct queue entries
- **AND** attempts each entry independently

### Requirement: Asynchronous Terminal-Outcome Receipt

When a queued message resolves to a non-delivered terminal outcome, relay SHALL
deliver a terminal-outcome receipt back to the original sender, out of band from
the accept-time response. The receipt SHALL be a relay-originated envelope
addressed to the sender and delivered through the sender's own transport via the
existing delivery pipeline, the same way any message reaches that session (a
Tmux pane, an ACP turn, a Pty write, or a UI stream frame). The receipt SHALL
carry the original `message_id`, the delivery target, the terminal outcome, and
any `reason_code`, so the sender can correlate it to the `queued` result it
received at accept time.

Receipts SHALL be delivered for non-delivered terminal outcomes only: `failed`
(including `reason_code = "pane_wedged"`), `timeout`, and `dropped_on_shutdown`.
A `delivered` outcome SHALL NOT produce a receipt; it is recorded per Async
Delivery Observability only.

A terminal-outcome receipt SHALL be relay/system-originated and SHALL NOT be
attributed to a peer principal, so a recipient can distinguish it from inbound
peer traffic.

A terminal-outcome receipt is itself a delivery and SHALL NOT produce a receipt
of its own; receipts are non-recursive. A receipt's own terminal outcome SHALL be
recorded per Async Delivery Observability and go no further.

Receipt delivery SHALL be best-effort. If the sender session is not routable at
terminal-resolution time, relay SHALL drop the receipt. Relay SHALL NOT persist,
queue indefinitely, or retry a dropped receipt; deferred delivery is out of
scope. The underlying terminal outcome SHALL still be recorded per Async Delivery
Observability regardless of whether the receipt is delivered.

`queued` SHALL denote async acceptance for delivery only. Relay SHALL NOT present
`queued` as a terminal `delivered`/success outcome, and the terminal outcome
SHALL be the authoritative result for a queued message.

#### Scenario: Deliver a non-delivered outcome receipt through the sender's transport

- **WHEN** a queued message to a target resolves as a non-delivered terminal
  outcome (`failed`, `timeout`, or `dropped_on_shutdown`)
- **AND** the original sender's session is routable
- **THEN** relay delivers a terminal-outcome receipt to the sender through the
  sender's own transport
- **AND** the receipt carries the original `message_id`, the delivery target, the
  terminal outcome, and any `reason_code`

#### Scenario: Deliver a wedged outcome receipt

- **WHEN** a queued message to a Tmux target resolves as `SendOutcome::Failed`
  with `reason_code = "pane_wedged"`
- **AND** the original sender's session is routable
- **THEN** relay delivers a terminal-outcome receipt naming that `message_id`,
  target, and `reason_code = "pane_wedged"` to the sender

#### Scenario: No receipt for a delivered outcome

- **WHEN** a queued message resolves as `delivered`
- **THEN** relay does not deliver a terminal-outcome receipt to the sender
- **AND** records the `delivered` outcome per Async Delivery Observability

#### Scenario: Drop receipt when the sender is not routable

- **WHEN** a queued message resolves to a non-delivered terminal outcome
- **AND** the original sender's session is not routable at resolution time
- **THEN** relay drops the receipt without persisting or retrying it
- **AND** relay still records the terminal outcome per Async Delivery
  Observability

#### Scenario: Receipts are not recursive

- **WHEN** a terminal-outcome receipt delivered to a sender itself reaches a
  terminal outcome
- **THEN** relay does not deliver a receipt for the receipt
- **AND** records the receipt's own terminal outcome per Async Delivery
  Observability

#### Scenario: Queued is not a terminal success signal

- **WHEN** a target is accepted for async delivery
- **THEN** the accept-time per-target result is `queued`
- **AND** `queued` is not presented as a terminal `delivered`/success outcome
- **AND** the authoritative outcome is the terminal outcome, delivered as a
  receipt when non-delivered

### Requirement: Async Delivery Observability

Relay SHALL emit inscriptions for async queue lifecycle transitions.

The terminal-outcome inscription SHALL cover every terminal outcome:
`delivered`, `failed` (including `reason_code = "pane_wedged"`), `timeout`, and
`dropped_on_shutdown`. This inscription SHALL be recorded regardless of whether a
terminal-outcome receipt is delivered to the sender, so `relay.log` is a complete
observability floor for terminal outcomes.

#### Scenario: Record queued async acceptance

- **WHEN** relay accepts an async target for queued delivery
- **THEN** relay writes an inscription event containing target session and
  message id with queued state

#### Scenario: Record terminal async outcome

- **WHEN** an async queued target reaches a terminal state (`delivered`,
  `failed`, `timeout`, or `dropped_on_shutdown`)
- **THEN** relay writes an inscription event containing target session,
  message id, and terminal outcome

#### Scenario: Record terminal outcome even when no receipt is delivered

- **WHEN** an async queued target reaches a terminal state
- **AND** no terminal-outcome receipt is delivered (the outcome is `delivered`,
  or the sender is not routable)
- **THEN** relay still writes the terminal-outcome inscription

### Requirement: Async Queue Growth Risk Disclosure

The system SHALL document that async queueing has no built-in hard cap and
may grow without bound if targets never become ready.

#### Scenario: Document unbounded queue risk for operators

- **WHEN** operator-facing documentation is updated for async delivery mode
- **THEN** it includes explicit guidance on unbounded pending queue risk
- **AND** suggests using `quiescence_timeout_ms` where bounded waits are needed
