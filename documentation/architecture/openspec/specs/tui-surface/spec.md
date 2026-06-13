# tui-surface Specification

## Purpose
TBD - created by archiving change add-tui-mvp-workbench. Update Purpose after archive.
## Requirements
### Requirement: Initial TUI MVP Workflow Coverage

The system SHALL define an initial TUI MVP that covers these operator
workflows:

- recipient discovery/selection,
- compose-and-send delivery,
- look snapshot inspection,
- delivery-events inspection and pending indicator.

The MVP SHALL reuse existing relay delivery and inspection semantics.
The MVP SHALL consume inbound message and delivery-outcome updates from relay
stream transport contracts defined in
`add-relay-stream-hello-transport-mvp`.

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

The TUI SHALL use context-sensitive `Tab` behavior in compose:
- when active recipient token in `To` has completion candidates, `Tab` initiates
  and cycles in-place completion proposals,
- when completion does not apply, `Tab` follows focus navigation behavior.

The TUI SHALL support accepting an active recipient completion proposal from
`To` via `Enter`.

The TUI SHALL support `@`-prefixed completion trigger behavior in `To`:
- once an `@` token has at least one character suffix, completion proposals
  update immediately without requiring an initial `Tab`.

The TUI SHALL provide a keyboard-opened recipient picker overlay (default
shortcut `F2`) that allows inserting recipients into `To`.

Function keys are reserved for overlay windows in MVP; completion behavior
SHALL NOT depend on `F4`.

#### Scenario: Use Tab for in-place recipient completion

- **WHEN** focus is in `To` and current token has completion candidates
- **AND** operator presses `Tab`
- **THEN** the TUI inserts or cycles a completion proposal in-place.

#### Scenario: Tab falls back to focus navigation when completion is inapplicable

- **WHEN** completion is inapplicable for active `To` token
- **AND** operator presses `Tab`
- **THEN** compose focus moves according to navigation rules.

#### Scenario: Trigger immediate proposals with @-prefixed token

- **WHEN** focus is in `To`
- **AND** active token starts with `@` and has one or more suffix characters
- **THEN** completion proposals update immediately without requiring initial `Tab`.

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

#### Scenario: Send requests use async mode in MVP

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
- qualified identifiers: `<bundle-id>/<session-id>` (reserved for future use)

MVP delivery/inspection behavior SHALL remain same-bundle-only.

Qualified identifiers implying cross-bundle scope SHALL be rejected in MVP with
unsupported-scope validation behavior.

#### Scenario: Accept local identifier in MVP

- **WHEN** an operator targets `<session-id>` in associated bundle context
- **THEN** the TUI treats that target as valid for send/look workflows

#### Scenario: Reject cross-bundle-qualified identifier in MVP

- **WHEN** an operator targets `<bundle-id>/<session-id>` outside associated
  bundle context
- **THEN** the TUI surfaces unsupported-scope validation feedback
- **AND** does not dispatch cross-bundle delivery/inspection behavior

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

### Requirement: Explicit MVP Non-Goals

The initial TUI MVP SHALL exclude:

- cross-bundle delivery/inspection implementation,
- multi-relay host-fleet orchestration UI,
- historical transcript/archive browsing,
- authorization model redesign,
- rich-editor extensions (attachments/templates/multi-buffer drafts),
- free-form `@mention` parser semantics.

#### Scenario: Defer free-form mention parser in MVP

- **WHEN** evaluating MVP compose behavior
- **THEN** recipient entry remains explicit `To` with deterministic IDs
- **AND** free-form mention parsing remains out of scope

### Requirement: TUI Sender Identity Precedence

`agentmux tui` SHALL resolve identity and bundle from global `users.toml`
configuration with deterministic precedence:

Sender/session resolution:

1. CLI `--session` when provided
2. `default-session` from global `users.toml`
3. fail-fast `validation_unknown_session`

Bundle resolution:

1. CLI `--bundle` when provided
2. `default-bundle` from global `users.toml`
3. fail-fast `validation_unknown_bundle`

`agentmux tui --sender` SHALL NOT be supported in MVP.

Association-derived sender fallback SHALL NOT be used for TUI startup in MVP.

TUI runtime SHALL use resolved session `id` consistently for
relay-backed operations in that process.
If selected session references unknown policy, startup SHALL fail fast with
`validation_unknown_policy`.

#### Scenario: Resolve TUI startup from explicit session/bundle selectors

- **WHEN** operator starts TUI with `--bundle agentmux --session user@GLOBAL`
- **AND** session `user@GLOBAL` is configured in global users
- **THEN** TUI resolves bundle `agentmux` and sender identity `user@GLOBAL`

#### Scenario: Resolve TUI startup from global defaults

- **WHEN** operator starts TUI without `--bundle`/`--session`
- **AND** global `users.toml` defines `default-bundle` and `default-session`
- **THEN** TUI resolves startup identity from those defaults

#### Scenario: Reject sender flag at startup

- **WHEN** operator starts TUI with `--sender relay`
- **THEN** startup fails with a stable validation error

#### Scenario: Fail when required defaults absent

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `users.toml`
- **THEN** startup fails with stable validation code

#### Scenario: Reject default session with unknown policy

- **WHEN** operator starts TUI without selectors
- **AND** `default-session` in `users.toml` references a policy that does not
  exist
- **THEN** startup fails with `validation_unknown_policy`

### Requirement: TUI Delivery State Mapping

TUI state and history surfaces SHALL use this outcome vocabulary:

- `accepted`
- `success`
- `timeout`
- `failed`

Mapping rules SHALL be:

- async send acceptance maps to `accepted`
- terminal delivered outcome maps to `success`
- terminal timeout outcome maps to `timeout`
- terminal non-timeout failure maps to `failed`
- terminal relay state `dropped_on_shutdown` maps to `failed` with
  `reason_code=dropped_on_shutdown`

`accepted` is process-local state derived from send acknowledgement and SHALL
NOT require replay after reconnect.

#### Scenario: Represent async send acceptance as accepted

- **WHEN** relay accepts an async send request for one or more targets
- **THEN** TUI records initial delivery state `accepted` for those targets

#### Scenario: Transition accepted state to terminal outcome

- **WHEN** TUI receives terminal delivery update for an accepted target
- **THEN** TUI updates state to exactly one of `success`, `timeout`, or
  `failed`

#### Scenario: Treat accepted state as local on reconnect

- **WHEN** TUI reconnects stream handling after process restart
- **THEN** TUI does not require replay of `accepted` lifecycle events
- **AND** applies terminal outcomes from relay stream events

### Requirement: TUI Transport Failure Semantics

TUI SHALL surface transport/connectivity failures explicitly and SHALL NOT
silently degrade into synthetic success states.

When startup transport is unavailable, TUI SHALL attempt runtime relay
auto-start before rendering an unavailable state.

Auto-started relay lifecycle remains external; TUI exit SHALL NOT auto-stop
relay.

#### Scenario: Surface relay connectivity failure explicitly

- **WHEN** relay transport is unavailable during TUI stream handling
- **THEN** TUI renders machine-readable transport error state
- **AND** does not report synthetic successful delivery/history updates

#### Scenario: Attempt relay auto-start on startup transport miss

- **WHEN** operator launches `agentmux tui`
- **AND** relay socket is unavailable at startup
- **THEN** TUI attempts runtime relay auto-start before declaring unavailable

#### Scenario: Do not auto-stop relay on tui exit

- **WHEN** relay was auto-started during TUI startup
- **AND** TUI process exits
- **THEN** TUI does not issue relay shutdown solely due to TUI exit

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

### Requirement: TUI raww accepted response handling

TUI raww accepted responses SHALL be treated as dispatch-accepted and SHALL NOT
be interpreted as terminal completion.

For ACP accepted responses with
`details.delivery_phase = "accepted_in_progress"`, TUI SHALL preserve the phase
indicator in status presentation where shown.

#### Scenario: Treat accepted_in_progress as non-terminal

- **WHEN** TUI receives raww response with `status = "accepted"`
- **AND** `details.delivery_phase = "accepted_in_progress"`
- **THEN** TUI marks request accepted at dispatch boundary
- **AND** does not mark terminal completion from that response alone

### Requirement: TUI Pending Permission Visibility

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

### Requirement: TUI Permission Decision Actions

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

### Requirement: Session-Scoped Permission Workflow

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

### Requirement: Permission Terminal State Updates

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

If the resolved session has any other type (`tmux`, `acp`, `pubsub`), the TUI
SHALL reject startup with a structured validation error rather than proceeding
with an incompatible delivery model.

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
placeholder with hint text directing the operator to open the picker.

`Interaction` mode SHALL NOT auto-open the picker on entry.

When an interaction target is selected, the look snapshot pane SHALL render
the same payload semantics as the current Look workflow (tmux line snapshot
or ACP structured entries) for that target.

#### Scenario: Interaction mode with no target shows placeholder

- **WHEN** operator switches to `Interaction` and no target is selected
- **THEN** the TUI renders an empty-target placeholder with hint text
- **AND** the TUI does not auto-open the picker overlay

#### Scenario: Interaction mode renders look snapshot for active target

- **WHEN** `Interaction` mode has active target `acp`
- **THEN** the look snapshot pane renders the snapshot payload for `acp`

### Requirement: Picker Mode-Switch Actions

The recipient picker overlay SHALL provide mode-switch actions that set the
`Interaction` mode target and switch the active mode to `Interaction`:

- `l` / `L` — set interaction target to the selected recipient and switch to
  `Interaction` mode
- `w` / `W` — set interaction target to the selected recipient, switch to
  `Interaction` mode, and place input focus on the raww input pane

Picker mode-switch actions SHALL NOT dispatch raww or capture look snapshots
directly from the picker.

#### Scenario: Picker l switches to Interaction with selected target

- **WHEN** operator opens the picker, selects recipient `acp`, and presses `l`
- **THEN** the picker closes
- **AND** the TUI switches to `Interaction` mode with active target `acp`

#### Scenario: Picker w switches to Interaction and focuses raww input

- **WHEN** operator opens the picker, selects recipient `acp`, and presses `w`
- **THEN** the picker closes
- **AND** the TUI switches to `Interaction` mode with active target `acp`
- **AND** input focus is on the raww input pane

#### Scenario: Picker w does not dispatch raww directly

- **WHEN** operator presses `w` in the picker
- **THEN** no raww request is dispatched to relay from the picker action
- **AND** raww dispatch requires explicit operator submission from the raww
  input pane in `Interaction` mode

### Requirement: Interaction Mode Permission/Raww Pane Replacement

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

