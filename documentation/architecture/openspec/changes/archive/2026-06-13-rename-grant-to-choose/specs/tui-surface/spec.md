## MODIFIED Requirements

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
