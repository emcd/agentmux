## ADDED Requirements

### Requirement: Initial TUI MVP Workflow Coverage

The system SHALL define an initial TUI MVP that covers these operator
workflows:

- recipient discovery/selection,
- compose-and-send delivery,
- look snapshot inspection,
- delivery-events inspection and pending indicator.

The MVP SHALL reuse existing relay and public command/tool semantics and SHALL
NOT introduce a new transport contract.

#### Scenario: Cover core operator loop in one TUI surface

- **WHEN** an operator uses the TUI for routine coordination
- **THEN** the TUI supports recipient selection, send, and look workflows
- **AND** renders per-target outcomes without requiring shell command hopping

### Requirement: Recipient Entry Model

The compose workflow SHALL use an explicit recipient field:

- `To`

The canonical send target state SHALL be deterministic recipient identifiers,
not free-form parsed prose.

#### Scenario: Build deterministic target set from To field

- **WHEN** an operator enters recipients in `To`
- **THEN** the TUI derives a deterministic target identifier set for send
- **AND** preserves `To` display semantics for operator context

### Requirement: Recipient Autocomplete and Picker Overlay

The TUI SHALL provide recipient completion from known identities in associated
bundle context.

The TUI SHALL use context-sensitive `Tab` behavior in compose:
- when active recipient token in `To` has completion candidates, `Tab` initiates
  and cycles in-place completion proposals,
- when completion does not apply, `Tab` follows focus navigation behavior.

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
