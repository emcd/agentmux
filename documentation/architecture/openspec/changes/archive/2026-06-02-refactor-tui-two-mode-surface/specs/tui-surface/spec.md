## ADDED Requirements

### Requirement: Screen Mode Model

The TUI SHALL define two top-level screen modes as peers:

- `Communication` — owns send/receive workflows (chat history and compose).
- `Interaction` — owns session-inspection workflows (look snapshot, raww
  dispatch input, and permission decisioning).

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
  position, raww input draft, and permission-row selection cursor

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
pane, and a raww-or-permission region.

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

The raww input pane and the permission decisioning pane SHALL share the same
screen region in `Interaction` mode.

Region occupancy rules:

- when the active interaction target has at least one pending permission
  request AND the raww input is empty, the permission decisioning pane
  occupies the region
- otherwise the raww input pane occupies the region

#### Scenario: Permission pane replaces empty raww input on pending request

- **WHEN** `Interaction` active target has at least one pending permission
  request
- **AND** raww input draft is empty
- **THEN** the permission decisioning pane occupies the raww region

#### Scenario: Raww keeps region while operator composes

- **WHEN** `Interaction` active target has at least one pending permission
  request
- **AND** raww input draft is non-empty
- **THEN** the raww input pane occupies the region

#### Scenario: Raww keeps region with no pending requests

- **WHEN** `Interaction` active target has no pending permission requests
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

## MODIFIED Requirements

### Requirement: Session-Scoped Permission Workflow

TUI SHALL provide a session-scoped permission workflow in `Interaction` mode
for the active interaction target session.

Workflow contract:

- pending rows in `Interaction` mode are filtered to the active interaction
  target session
- selection for multiple pending requests is deterministic by relay FIFO order
- action hints and empty-state text are visible in `Interaction` mode

#### Scenario: Show session-scoped pending requests in Interaction mode

- **WHEN** operator is in `Interaction` mode with active target session `acp`
- **AND** pending permissions exist for sessions `acp` and `relay`
- **THEN** Interaction-mode permission actions render only pending requests
  for `acp`
