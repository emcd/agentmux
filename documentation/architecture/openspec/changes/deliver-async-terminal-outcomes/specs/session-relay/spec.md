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
window), the Tmux transport SHALL resolve every sender in the flush group
with `SendOutcome::Timeout`. The `Timeout` outcome SHALL remain a distinct
terminal outcome and SHALL NOT be collapsed into `Failed`. As a non-delivered
outcome it SHALL be surfaced to the sender through the Asynchronous
Terminal-Outcome Receipt and recorded per Async Delivery Observability; it SHALL
NOT be returned in the synchronous accept-time response.

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
