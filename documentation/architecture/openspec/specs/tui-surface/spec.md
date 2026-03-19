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

`agentmux tui` sender identity SHALL resolve with deterministic precedence:

1. CLI `--sender` when provided
2. local testing override `tui.toml` sender when active
3. normal `<config-root>/tui.toml` sender
4. runtime association-derived sender identity
5. fail-fast validation error when unresolved

TUI runtime SHALL use the resolved sender identity consistently for relay-backed
operations in that TUI process.

#### Scenario: Prefer CLI sender over configured sender files

- **WHEN** operator starts TUI with `--sender`
- **AND** sender is also configured via override or normal `tui.toml`
- **THEN** TUI resolves sender from `--sender`

#### Scenario: Fail fast when sender cannot be resolved

- **WHEN** CLI sender is absent
- **AND** configured sender files are absent or inapplicable
- **AND** runtime association cannot resolve a valid sender
- **THEN** TUI startup fails with stable validation error code

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

Same-bundle MVP lock SHALL remain enforced for transport and history updates.

#### Scenario: Surface relay connectivity failure explicitly

- **WHEN** relay transport is unavailable during TUI stream handling
- **THEN** TUI renders machine-readable transport error state
- **AND** does not report synthetic successful delivery/history updates

#### Scenario: Reject cross-bundle transport scope in MVP

- **WHEN** TUI transport scope attempts bundle outside associated context
- **THEN** request is rejected with `validation_cross_bundle_unsupported`

