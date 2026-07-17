## ADDED Requirements

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

## MODIFIED Requirements

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
