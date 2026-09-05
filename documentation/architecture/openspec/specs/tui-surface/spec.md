# tui-surface Specification

## Purpose
The TUI surface contracts for the two-mode (Communication/Interaction) workbench. The spec governs recipient entry/autocomplete/picker overlay behavior (canonical target identifiers only; configured session `name` is not a send-target token), async delivery event inspection with the pending-deliveries indicator, the session-scoped choice decisioning workflow, snapshot/replay dedupe by `choice_request_id`, and mode-switch key bindings (F4 toggles Communication/Interaction with per-mode state preservation). TUI sender identity precedence against `users.toml` defaults and TUI raww dispatch/queued-response handling are also normative.
## Requirements
### Requirement: Initial TUI Workflow Coverage

The system SHALL define an initial TUI that covers these operator
workflows:

- recipient discovery/selection,
- compose-and-send delivery,
- look snapshot inspection,
- delivery-events inspection and pending indicator.

The TUI SHALL reuse existing relay delivery and inspection semantics.
The TUI SHALL consume inbound message and delivery-outcome updates from the
relay stream transport contracts specified by the `look-and-stream-events`
capability's `Relay Stream Event Contract` and `Hello Registration Contract`
requirements.

#### Scenario: Cover core operator loop with structured update flow

- **WHEN** an operator uses the TUI for routine coordination
- **THEN** the TUI supports recipient selection, send, look, and update
  workflows
- **AND** inbound message and delivery outcome updates are represented using
  canonical relay stream event payloads

### Requirement: Recipient Entry Model

The compose workflow SHALL use an explicit recipient field:

- `To`

The canonical send target state SHALL be deterministic recipient identifiers,
not free-form parsed prose.

TUI send submission SHALL use canonical target identifiers only.
Configured session display names are presentation/search artifacts and SHALL NOT
be submitted to relay as explicit send target tokens.

#### Scenario: Build deterministic target set from To field

- **WHEN** an operator enters recipients in `To`
- **THEN** the TUI derives a deterministic target identifier set for send
- **AND** preserves `To` display semantics for operator context

#### Scenario: Submit canonical identifiers instead of display-name tokens

- **WHEN** an operator selects a recipient via name-oriented completion/picker
- **THEN** TUI submits canonical identifier tokens for send
- **AND** does not submit display-name tokens directly

### Requirement: Recipient Autocomplete and Picker Overlay

The TUI SHALL provide recipient completion from known identities in associated
bundle context.

The TUI SHALL use `Ctrl+Space` for manual recipient completion in compose:
- when active recipient token in `To` has completion candidates, `Ctrl+Space`
  initiates in-place completion proposals,
- `Tab` follows focus navigation behavior.

The TUI SHALL support accepting an active recipient completion proposal from
`To` via `Enter`.

The TUI SHALL support `@`-prefixed completion trigger behavior in `To`:
- once an `@` token has at least one character suffix, completion proposals
  update immediately without requiring an initial `Ctrl+Space`.

The TUI SHALL provide a keyboard-opened recipient picker overlay (default
shortcut `F2`) that allows inserting recipients into `To`.

Function keys are reserved for overlay windows. Completion behavior
SHALL NOT depend on `F4`.

#### Scenario: Use Ctrl+Space for in-place recipient completion

- **WHEN** focus is in `To` and current token has completion candidates
- **AND** operator presses `Ctrl+Space`
- **THEN** the TUI inserts a completion proposal in-place.

#### Scenario: Tab follows focus navigation

- **WHEN** operator presses `Tab` in compose
- **THEN** compose focus moves according to navigation rules.

#### Scenario: Trigger immediate proposals with @-prefixed token

- **WHEN** focus is in `To`
- **AND** active token starts with `@` and has one or more suffix characters
- **THEN** completion proposals update immediately without requiring initial
  `Ctrl+Space`.

#### Scenario: Accept active completion with Enter in To

- **WHEN** focus is in `To`
- **AND** a completion proposal is active for the current recipient token
- **AND** operator presses `Enter`
- **THEN** the active completion proposal is accepted for that token.

#### Scenario: Insert recipients from overlay picker

- **WHEN** an operator opens the recipient picker overlay
- **AND** selects one or more recipients
- **THEN** the TUI inserts those recipients into `To`

### Requirement: Async Delivery Events and Pending Indicator

The TUI SHALL submit send actions using async delivery behavior.

The TUI SHALL provide a delivery events overlay (default shortcut `F3`) for
outcome visibility and SHALL expose a pending-deliveries indicator in the main
surface status context.

#### Scenario: Send requests use async mode

- **WHEN** an operator sends a message from TUI
- **THEN** the relay request uses async delivery behavior
- **AND** no delivery-mode toggle is exposed in TUI.

#### Scenario: Delivery events and pending count update on send responses

- **WHEN** a send response includes per-target delivery outcomes
- **THEN** the TUI appends event entries to the events overlay history
- **AND** updates pending-deliveries indicator using available outcome data.

### Requirement: Forward-Compatible Target Identifier Grammar

The TUI target identifier grammar SHALL support:

- local identifiers: `<session-id>`
- canonical identifiers: `<session-id>@<namespace>`

The TUI SHALL submit relay targets as canonical `session@namespace` principal
identifiers. Bare local identifiers SHALL be qualified with the sender's bound
bundle before dispatch. Relay-wide senders without a bound bundle SHALL reject
bare identifiers with `validation_unqualified_target`.

Cross-namespace delivery and inspection SHALL use canonical `session@namespace`
targets and rely on relay authorization for the requested operation.

#### Scenario: Accept local identifier

- **WHEN** an operator targets `<session-id>` in associated bundle context
- **THEN** the TUI qualifies it as `<session-id>@<bound-bundle>` for relay
  dispatch

#### Scenario: Accept canonical cross-namespace identifier

- **WHEN** an operator targets `<session-id>@<namespace>`
- **THEN** the TUI submits that canonical identifier to relay unchanged
- **AND** relay authorization determines whether the operation may reach that
  namespace

#### Scenario: Reject bare target for relay-wide sender

- **WHEN** a relay-wide sender targets bare `<session-id>`
- **THEN** the TUI rejects the target with `validation_unqualified_target`
- **AND** requires an explicit `<session-id>@<namespace>` identifier

### Requirement: Contract and Error Taxonomy Fidelity

TUI send and look actions SHALL map to existing relay-backed semantics:

- send uses delivery behavior aligned with `send` contract
- look uses payload semantics aligned with `look` contract

The TUI SHALL preserve stable machine-readable validation/error codes in
operator-visible error rendering.

#### Scenario: Surface stable validation code for invalid look lines

- **WHEN** look invocation fails with `validation_invalid_lines`
- **THEN** the TUI error surface includes that stable validation code

#### Scenario: Surface stable validation code for unknown target

- **WHEN** send or look invocation fails with `validation_unknown_target`
- **THEN** the TUI error surface includes that stable validation code

### Requirement: Explicit Non-Goals

The TUI SHALL exclude:

- multi-relay host-fleet orchestration UI,
- historical transcript/archive browsing,
- authorization model redesign,
- rich-editor extensions (attachments/templates/multi-buffer drafts),
- free-form `@mention` parser semantics.

#### Scenario: Defer free-form mention parser

- **WHEN** evaluating compose behavior
- **THEN** recipient entry remains explicit `To` with deterministic IDs
- **AND** free-form mention parsing remains out of scope

### Requirement: TUI Sender Identity Precedence

`agentmux tui` SHALL resolve identity and the browsing bundle from TUI
configuration with deterministic precedence:

Sender/session resolution:

1. CLI `--as-session` when provided
2. `default-session` from `users.toml`
3. fail-fast `validation_unknown_session`

Browsing bundle resolution:

1. CLI `--bundle` when provided
2. `default-bundle` from `ui.toml`
3. first available configured bundle
4. empty browsing context when no bundle is available

Association-derived sender fallback SHALL NOT be used for TUI startup.

TUI runtime SHALL use resolved session `id` consistently for
relay-backed operations in that process.
If selected session references unknown policy, startup SHALL fail fast with
`validation_unknown_policy`.

#### Scenario: Resolve TUI startup from explicit selectors

- **WHEN** operator starts TUI with `--bundle agentmux --as-session user@GLOBAL`
- **AND** session `user@GLOBAL` is configured in active TUI configuration
- **THEN** TUI resolves browsing bundle `agentmux` and sender identity
  `user@GLOBAL`

#### Scenario: Resolve TUI startup from global defaults

- **WHEN** operator starts TUI without explicit selectors
- **AND** `ui.toml` defines `default-bundle` and `users.toml` defines
  `default-session`
- **THEN** TUI resolves startup identity from those defaults

#### Scenario: Allow startup without bundle default

- **WHEN** operator starts TUI without `--bundle`
- **AND** `ui.toml` does not define `default-bundle`
- **THEN** TUI resolves the browsing bundle from the first available configured
  bundle
- **AND** if no bundle is available, TUI starts with an empty browsing context

#### Scenario: Fail when required session selector is absent

- **WHEN** operator starts TUI without `--as-session`
- **AND** `users.toml` does not define `default-session`
- **THEN** startup fails with `validation_unknown_session`

#### Scenario: Reject default session with unknown policy

- **WHEN** operator starts TUI without selectors
- **AND** `default-session` in `users.toml` references a policy that does not
  exist
- **THEN** startup fails with `validation_unknown_policy`

### Requirement: TUI Delivery State Mapping

TUI state and history surfaces SHALL use this outcome vocabulary:

- `accepted`
- `success`
- `failed`
- `not_submitted`
- `submission_unknown`

Mapping rules SHALL be:

- async send acceptance maps to `accepted`
- terminal delivered outcome maps to `success`
- terminal delivery failure maps to `failed`
- terminal relay state `dropped_on_shutdown` maps to `failed` with
  `reason_code=dropped_on_shutdown`
- terminal `not_submitted` maps to `not_submitted`
- terminal `submission_unknown` maps to `submission_unknown`

`not_submitted` and `submission_unknown` SHALL be represented distinctly rather
than collapsed into `failed` or into an unknown-outcome placeholder. They carry
opposite evidentiary claims — one asserts that no target-side effect occurred,
the other that such an effect cannot be excluded — and a surface that renders
either as a failure asserts a non-delivery the relay cannot support.

`accepted` is process-local state derived from send acknowledgement and SHALL
NOT require replay after reconnect.

A terminal outcome the TUI does not recognize SHALL be surfaced as an explicit
unknown-outcome placeholder rather than mapped to any of the above.

#### Scenario: Represent async send acceptance as accepted

- **WHEN** relay accepts an async send request for one or more targets
- **THEN** TUI records initial delivery state `accepted` for those targets

#### Scenario: Transition accepted state to terminal outcome

- **WHEN** TUI receives terminal delivery update for an accepted target
- **THEN** TUI updates state to exactly one of `success`, `failed`,
  `not_submitted`, or `submission_unknown`

#### Scenario: Represent an evidence-bearing terminal outcome distinctly

- **WHEN** TUI receives a terminal delivery outcome of `not_submitted` or
  `submission_unknown` from a relay stream event
- **THEN** TUI records that outcome under its own spelling
- **AND** does not render it as `failed` or as an unknown-outcome placeholder

#### Scenario: Treat accepted state as local on reconnect

- **WHEN** TUI reconnects stream handling after process restart
- **THEN** TUI does not require replay of `accepted` lifecycle events
- **AND** applies terminal outcomes from relay stream events

### Requirement: TUI Transport Failure Semantics

TUI SHALL surface transport/connectivity failures explicitly and SHALL NOT
silently degrade into synthetic success states.

When startup transport is unavailable, TUI SHALL attempt runtime relay
auto-start before rendering an unavailable state.

When TUI auto-spawns a relay during startup, TUI exit SHALL terminate that
auto-spawned relay through the relay's graceful shutdown path. A relay that was
already running at TUI startup SHALL remain untouched on TUI exit.

#### Scenario: Surface relay connectivity failure explicitly

- **WHEN** relay transport is unavailable during TUI stream handling
- **THEN** TUI renders machine-readable transport error state
- **AND** does not report synthetic successful delivery/history updates

#### Scenario: Attempt relay auto-start on startup transport miss

- **WHEN** operator launches `agentmux tui`
- **AND** relay socket is unavailable at startup
- **THEN** TUI attempts runtime relay auto-start before declaring unavailable

#### Scenario: Stop auto-spawned relay on tui exit

- **WHEN** relay was auto-spawned during TUI startup
- **AND** TUI process exits
- **THEN** TUI issues a graceful relay shutdown for the relay it spawned

#### Scenario: Leave already-running relay on tui exit

- **WHEN** relay was already running at TUI startup
- **AND** TUI process exits
- **THEN** TUI does not issue relay shutdown

### Requirement: TUI raww dispatch contract

TUI raw write actions SHALL dispatch through relay raww contract and SHALL NOT
perform transport-specific writes directly.

TUI raww requests SHALL include:
- `target_session`
- `text`
- optional `no_enter` (default `false`)

#### Scenario: Dispatch raww through relay contract

- **WHEN** operator triggers raw write from TUI
- **THEN** TUI submits raww request through relay operation
- **AND** does not call tmux/ACP transport directly from UI layer

### Requirement: TUI raww error handling taxonomy

TUI raww failure handling SHALL treat canonical relay codes as terminal,
including:

- `validation_unknown_target`
- `validation_unsupported_operation`
- `validation_invalid_params`
- `authorization_forbidden`

#### Scenario: Show deterministic validation error for unsupported target class

- **WHEN** relay returns `validation_unsupported_operation` for an unsupported
  raww target class
- **THEN** TUI surfaces the error as terminal without retry

### Requirement: TUI raww queued response handling

TUI raww queued responses SHALL be treated as enqueue-accepted and SHALL NOT
be interpreted as terminal completion. The terminal delivery outcome arrives
out-of-band via a `delivery_outcome` stream event keyed by `message_id`.

TUI SHALL enroll the raww `message_id` into pending delivery tracking at
enqueue time so the later `delivery_outcome` event closes it out.

#### Scenario: Treat queued response as non-terminal

- **WHEN** TUI receives raww response with `status = "queued"`
- **THEN** TUI marks request accepted at dispatch boundary
- **AND** enrolls `message_id` into pending delivery tracking
- **AND** does not mark terminal completion until `delivery_outcome` event arrives

### Requirement: TUI Pending Choice Visibility

TUI SHALL expose pending ACP choice requests received from canonical relay
lifecycle events.

Pending list entries SHALL be keyed by `choice_request_id` and include
request context sufficient for operator decisioning, including:

- `message_id`
- `target_session`
- `requested_kind`
- `requested_details`
- `enqueued_at`
- ACP choice `options` for explicit operator selection

#### Scenario: Render pending request from relay choices event

- **WHEN** relay emits `choices.requested`
- **THEN** TUI adds or updates a pending row keyed by `choice_request_id`

### Requirement: Snapshot and Replay Dedupe Contract

On connect/reconnect, TUI SHALL consume `choices.snapshot` plus replayed
`choices.requested` events using dedupe by `choice_request_id` so
at-least-once replay does not create duplicate pending rows.

#### Scenario: Avoid duplicate pending rows after snapshot replay

- **WHEN** TUI receives `choices.snapshot`
- **AND** relay replays matching `choices.requested` events
- **THEN** TUI keeps one pending row per `choice_request_id`

### Requirement: TUI Choice Decision Actions

TUI SHALL expose deterministic decision actions keyed by `choice_request_id`.

Action payload contract:

- `choices.pick { choice_request_id, outcome, option_id? }`
- allowed outcomes are `selected` and `cancelled`
- `selected` requires `option_id`
- `cancelled` must omit `option_id`

TUI SHALL NOT send caller-supplied actor identity fields in action payload.

#### Scenario: Submit selected action without actor spoof fields

- **WHEN** operator chooses a choice option from pending request
- **THEN** TUI submits `choices.pick` with
  `choice_request_id`, `outcome=selected`, and explicit `option_id`

#### Scenario: Submit cancelled action without option id

- **WHEN** operator cancels a pending choice request
- **THEN** TUI submits `choices.pick` with
  `choice_request_id` and `outcome=cancelled`

### Requirement: Session-Scoped Choice Workflow

TUI SHALL provide a session-scoped choice workflow in `Interaction` mode
for the active interaction target session.

Workflow contract:

- pending rows in `Interaction` mode are filtered to the active interaction
  target session
- selection for multiple pending requests is deterministic by relay FIFO order
- action hints and empty-state text are visible in `Interaction` mode

#### Scenario: Show session-scoped pending requests in Interaction mode

- **WHEN** operator is in `Interaction` mode with active target session `acp`
- **AND** pending choices exist for sessions `acp` and `relay`
- **THEN** Interaction-mode choice actions render only pending requests
  for `acp`

### Requirement: Choice Terminal State Updates

TUI SHALL apply terminal updates from `choices.resolved` and remove pending
entries deterministically by `choice_request_id`.

TUI-facing terminal vocabulary SHOULD align to:

- `selected`
- `cancelled`

#### Scenario: Remove pending item on resolved event

- **WHEN** relay emits `choices.resolved` for pending request
- **THEN** TUI marks terminal status and clears pending row for that id

### Requirement: TUI Session Type Validation

`agentmux tui --as-session X` SHALL fail fast when session `X` is not
configured with session type `ui`.

If the resolved session has any type other than `ui`, the TUI SHALL reject
startup with a structured validation error rather than proceeding with an
incompatible delivery model. The rejection SHALL follow from the resolved type
not being `ui`, not from membership in a list of rejected types, so a session
type added to `Session Type Taxonomy` later is rejected without amending this
requirement.

#### Scenario: Reject --as-session with non-ui session type

- **WHEN** operator starts TUI with `--as-session relay`
- **AND** session `relay` is a coder-backed session (resolved type `tmux`)
- **THEN** startup fails with a structured validation error indicating type
  mismatch

#### Scenario: Accept --as-session with ui session type

- **WHEN** operator starts TUI with `--as-session user@GLOBAL`
- **AND** session `user@GLOBAL` is configured with `[sessions.ui]`
- **THEN** TUI startup proceeds normally

### Requirement: Screen Mode Model

The TUI SHALL define two top-level screen modes as peers:

- `Communication` — owns send/receive workflows (chat history and compose).
- `Interaction` — owns session-inspection workflows (look snapshot, raww
  dispatch input, and choice decisioning).

Exactly one mode SHALL be active at any time.

Default mode at TUI startup SHALL be `Communication`.

#### Scenario: Default startup mode is Communication

- **WHEN** an operator launches `agentmux tui`
- **THEN** the TUI starts in `Communication` mode

#### Scenario: Exactly one mode active at a time

- **WHEN** the TUI is running
- **THEN** the operator-visible surface renders panes owned by exactly one of
  `Communication` or `Interaction`

### Requirement: Mode Switch Action

The TUI SHALL provide a deterministic mode-switch key binding (`F4`) that
toggles between `Communication` and `Interaction`.

A mode indicator SHALL be visible to the operator (footer region) in both
modes.

Per-mode state SHALL be preserved across switches:

- `Communication` retains compose draft text, focus field, and chat history
  scroll position
- `Interaction` retains active interaction target, look snapshot scroll
  position, raww input draft, and choice-row selection cursor

#### Scenario: F4 toggles between modes

- **WHEN** operator is in `Communication` and presses `F4`
- **THEN** the TUI switches to `Interaction`
- **AND** when operator presses `F4` again
- **THEN** the TUI switches back to `Communication`

#### Scenario: Per-mode draft state survives mode switch

- **WHEN** operator types a draft in `Communication` compose, switches to
  `Interaction`, then switches back
- **THEN** the compose draft text and cursor position are unchanged

#### Scenario: Mode indicator visible

- **WHEN** the TUI renders the footer in either mode
- **THEN** the footer indicates which mode is active

### Requirement: Communication Mode Surface

`Communication` mode SHALL render the chat history and compose (To and
Message) panes that today's workbench surface provides, and SHALL preserve
existing recipient autocomplete, picker overlay entry, async delivery, and
delivery state vocabulary semantics in that mode.

#### Scenario: Communication mode renders chat and compose

- **WHEN** the TUI is in `Communication` mode
- **THEN** the main surface renders chat history and compose (To and Message)
  panes
- **AND** the operator can send messages and inspect chat history as in the
  current workbench

### Requirement: Interaction Mode Surface

`Interaction` mode SHALL render an active-target header, a look snapshot
pane, and a raww-or-choice region.

`Interaction` mode SHALL maintain an active interaction target session.
When no interaction target is selected, the mode SHALL render an empty-target
placeholder with hint text directing the operator to open the picker and
choose a session.

When `Interaction` mode is entered with no interaction target selected, the
TUI SHALL auto-open the recipient picker focused on the session column so the
operator can choose a target immediately. Dismissing the picker without a
selection returns to the empty-target placeholder.

When an interaction target is selected, the look snapshot pane SHALL render
the same payload semantics as the current Look workflow (tmux line snapshot
or ACP structured entries) for that target.

On entering `Interaction` with an interaction target already selected, the TUI
SHALL re-capture that target's look snapshot rather than render the buffer
frozen from a prior visit. Re-capture SHALL preserve the operator's snapshot
scroll position and SHALL surface a relay failure to the operator.

#### Scenario: Interaction mode with no target auto-opens the picker

- **WHEN** operator switches to `Interaction` and no target is selected
- **THEN** the TUI auto-opens the recipient picker focused on the session column
- **AND** dismissing the picker without a selection shows the empty-target
  placeholder with hint text

#### Scenario: Interaction mode renders look snapshot for active target

- **WHEN** `Interaction` mode has active target `acp`
- **THEN** the look snapshot pane renders the snapshot payload for `acp`

#### Scenario: Re-entering Interaction refreshes the look snapshot

- **WHEN** operator re-enters `Interaction` with a previously selected target
- **THEN** the TUI re-captures that target's look snapshot before rendering
  rather than showing the buffer from the prior visit

### Requirement: Picker Session Selection Actions

The recipient picker overlay session column SHALL provide a mode-aware `Enter`
action that commits the selected recipient according to the active mode:

- In `Communication` mode, `Enter` inserts the selected recipient into the
  `To` field. Insertion SHALL NOT depend on which compose field holds focus:
  the picker is the recipient affordance and `To` is the only field a recipient
  can occupy, so opening the picker while composing the message body still
  inserts into `To` and leaves compose focus unchanged.
- In `Interaction` mode, `Enter` sets the interaction target to the selected
  recipient, synchronously captures that target's look snapshot via the relay
  `Look` operation, and closes the picker so the operator reaches the populated
  Interaction surface.

Picker session selection SHALL NOT dispatch raww directly; raww dispatch
requires explicit operator submission from the raww input pane in `Interaction`
mode.

#### Scenario: Picker Enter inserts recipient in Communication mode

- **WHEN** operator opens the picker in `Communication` mode, selects recipient
  `acp`, and presses `Enter` on the session column
- **THEN** the recipient `acp` is inserted into the `To` field

#### Scenario: Picker Enter inserts recipient while composing the message body

- **WHEN** operator is focused on the message body in `Communication` mode,
  opens the picker, selects recipient `acp`, and presses `Enter` on the session
  column
- **THEN** the recipient `acp` is inserted into the `To` field
- **AND** compose focus remains on the message body

#### Scenario: Picker Enter enters Interaction with a synchronous look snapshot

- **WHEN** operator opens the picker in `Interaction` mode, selects recipient
  `acp`, and presses `Enter` on the session column
- **THEN** the picker closes
- **AND** the TUI sets active target `acp` and captures its look snapshot
  synchronously before the operator reaches the raww input pane

#### Scenario: Picker selection does not dispatch raww directly

- **WHEN** operator selects a recipient in the picker
- **THEN** no raww request is dispatched to relay from the picker action
- **AND** raww dispatch requires explicit operator submission from the raww
  input pane in `Interaction` mode

### Requirement: Interaction Mode Choice/Raww Pane Replacement

The raww input pane and the choice decisioning pane SHALL share the same
screen region in `Interaction` mode.

Region occupancy rules:

- when the active interaction target has at least one pending choice request
  AND the raww input is empty, the choice decisioning pane occupies the region
- otherwise the raww input pane occupies the region

#### Scenario: Choice pane replaces empty raww input on pending request

- **WHEN** `Interaction` active target has at least one pending choice request
- **AND** raww input draft is empty
- **THEN** the choice decisioning pane occupies the raww region

#### Scenario: Raww keeps region while operator composes

- **WHEN** `Interaction` active target has at least one pending choice request
- **AND** raww input draft is non-empty
- **THEN** the raww input pane occupies the region

#### Scenario: Raww keeps region with no pending requests

- **WHEN** `Interaction` active target has no pending choice requests
- **THEN** the raww input pane occupies the region

### Requirement: Overlay Availability Across Modes

Overlays SHALL remain available in both `Communication` and `Interaction`
modes: help (`F1`), recipient picker (`F2`), and delivery events (`F3`).

Overlays SHALL render on top of whichever mode is active and SHALL NOT change
the active mode.

#### Scenario: F2 picker opens in Interaction mode

- **WHEN** operator is in `Interaction` mode and presses `F2`
- **THEN** the picker overlay opens over the `Interaction` surface
- **AND** the active mode remains `Interaction` while the picker is open

#### Scenario: F3 events overlay opens in either mode

- **WHEN** operator presses `F3` in either mode
- **THEN** the events overlay opens over the active mode surface

### Requirement: Keyboard Enhancement Capability Detection

The TUI SHALL probe the terminal once for progressive keyboard enhancement
(the Kitty keyboard protocol) after terminal setup and before the event loop
reads any key event, so the probe's query reply is not consumed by the loop's
input drain.

The probe SHALL resolve to exactly one of three outcomes:

- `Active` — the terminal advertised the protocol and the TUI pushed
  `DISAMBIGUATE_ESCAPE_CODES`,
- `Unsupported` — the terminal answered the probe without advertising the
  protocol,
- `ProbeFailed` — the probe did not complete (no controlling terminal, I/O
  failure, or no reply before the query timeout).

`ProbeFailed` SHALL be reported distinctly from `Unsupported`; the TUI SHALL
NOT collapse an unanswered probe into a negative answer.

The TUI SHALL push only `DISAMBIGUATE_ESCAPE_CODES`. It SHALL NOT push flags
that change which events are delivered (`REPORT_EVENT_TYPES`,
`REPORT_ALTERNATE_KEYS`, `REPORT_ALL_KEYS_AS_ESCAPE_CODES`) while no input
handler consumes them.

Pushed flags SHALL be popped before the terminal is restored, so the terminal
is left in the key-reporting mode it had before launch.

The TUI SHALL activate disambiguation where the terminal offers it so that
modified chords are deliverable and therefore bindable, widening the space of
chords the binding table can name. Activation SHALL NOT by itself change what
any chord does.

The probe outcome SHALL be visible to the operator in the help overlay, because
it reports what the TUI was able to determine: that disambiguation is active,
that the terminal answered without offering it, or that the determination could
not be made. The report SHALL describe the determination, never assert a
terminal limitation the probe did not establish.

That visibility SHALL hold at a 120-column, 24-row terminal: the report SHALL be
on screen when the overlay opens there, without the operator scrolling or
resizing. Unqualified, the requirement was satisfiable at no terminal size in
particular; the generated overlay does not fit 24 rows, and the report is the
part a viewport would otherwise leave off. The rest of the overlay is reachable
by scrolling, which `tui-action-bindings` governs.

The report for `ProbeFailed` SHALL NOT assert that the terminal lacks the
protocol. An unanswered probe establishes only that the TUI could not
determine or enable disambiguation.

Detection SHALL NOT itself introduce, remove, or reassign any key binding; it
bears only on whether a chord is deliverable, never on what a delivered chord
does. Which action a delivered chord invokes SHALL be declared per context in
the binding table, so no context acquires a modified-`Enter` behavior by
omitting a modifier condition.

The default bindings SHALL be capability-neutral: the observable result of any
chord SHALL NOT depend on the probe outcome. A modified `Enter` delivered
distinctly under `Active` SHALL invoke the same action it invokes under
`Unsupported` and `ProbeFailed`, where it is physically indistinguishable from
a bare `Enter`.

`Ctrl+J` SHALL insert a newline under every outcome.

#### Scenario: Modified Enter behaves the same under every probe outcome

- **WHEN** the operator presses `Shift+Enter` in the `Message` field
- **THEN** the message is sent
- **AND** the result is the same whether the protocol is active, unsupported, or
  the probe failed

#### Scenario: Activation alone changes no behavior

- **WHEN** the protocol is active and disambiguation flags are pushed
- **THEN** every chord invokes the action it invokes without disambiguation
- **AND** the operator observes no behavior difference attributable to the probe

#### Scenario: Probe failure claims nothing about the terminal

- **WHEN** the probe does not complete
- **THEN** the operator-facing report states that the capability is
  undetermined
- **AND** it does not state that the terminal lacks the protocol

#### Scenario: Capable terminal activates disambiguation

- **WHEN** the terminal advertises progressive keyboard enhancement at TUI
  startup
- **THEN** the TUI pushes `DISAMBIGUATE_ESCAPE_CODES`
- **AND** the help overlay reports the protocol as active
- **AND** the flags are popped before the terminal is restored

#### Scenario: Terminal answers without advertising support

- **WHEN** the terminal answers the probe without advertising the protocol
- **THEN** the TUI pushes no enhancement flags
- **AND** the help overlay reports the protocol as unsupported

#### Scenario: Unanswered probe is distinct from an unsupported terminal

- **WHEN** the keyboard-enhancement probe does not complete
- **THEN** the outcome is reported as a probe failure
- **AND** that report is distinguishable from the report for a terminal that
  answered without advertising support

#### Scenario: Newline binding is unchanged by the probe outcome

- **WHEN** the operator inserts a newline in `Message` or the write input
- **THEN** `Ctrl+J` inserts the newline regardless of the probe outcome

#### Scenario: The probe outcome is on screen at a standard terminal size

- **WHEN** the operator opens the help overlay at a 120-column, 24-row terminal
- **THEN** the keyboard-enhancement report is on screen
- **AND** reaching it requires no scrolling and no resizing
