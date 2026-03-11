## ADDED Requirements

### Requirement: Initial TUI MVP Workflow Coverage

The system SHALL define an initial TUI MVP that covers these operator
workflows:

- recipient discovery/selection,
- compose-and-send delivery,
- look snapshot inspection,
- delivery outcome feedback.

The MVP SHALL reuse existing relay and public command/tool semantics and SHALL
NOT introduce a new transport contract.

#### Scenario: Cover core operator loop in one TUI surface

- **WHEN** an operator uses the TUI for routine coordination
- **THEN** the TUI supports recipient selection, send, and look workflows
- **AND** renders per-target outcomes without requiring shell command hopping

### Requirement: Recipient Entry Model

The compose workflow SHALL use explicit recipient fields:

- `To`
- `Cc`

The canonical send target state SHALL be deterministic recipient identifiers,
not free-form parsed prose.

#### Scenario: Build deterministic target set from To/Cc fields

- **WHEN** an operator enters recipients in `To` and/or `Cc`
- **THEN** the TUI derives a deterministic target identifier set for send
- **AND** preserves `To`/`Cc` display semantics for operator context

### Requirement: Recipient Autocomplete and Picker Overlay

The TUI SHALL provide recipient completion from known identities in associated
bundle context.

The TUI SHALL support keyboard completion in recipient fields via `Tab`.

The TUI SHALL provide a keyboard-opened recipient picker overlay (default
shortcut `F2`) that allows inserting recipients into `To`/`Cc`.

#### Scenario: Complete recipient with Tab

- **WHEN** focus is in a recipient field and operator presses `Tab`
- **THEN** the TUI completes using current bundle recipient identities

#### Scenario: Insert recipients from overlay picker

- **WHEN** an operator opens the recipient picker overlay
- **AND** selects one or more recipients
- **THEN** the TUI inserts those recipients into the active recipient field

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
- **THEN** recipient entry remains explicit `To`/`Cc` with deterministic IDs
- **AND** free-form mention parsing remains out of scope
