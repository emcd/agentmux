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

`agentmux tui` SHALL resolve identity and bundle from global `tui.toml`
configuration with deterministic precedence:

Sender/session resolution:

1. CLI `--session` when provided
2. `default-session` from global `tui.toml`
3. fail-fast `validation_unknown_session`

Bundle resolution:

1. CLI `--bundle` when provided
2. `default-bundle` from global `tui.toml`
3. fail-fast `validation_unknown_bundle`

`agentmux tui --sender` SHALL NOT be supported in MVP.

Association-derived sender fallback SHALL NOT be used for TUI startup in MVP.

TUI runtime SHALL use resolved session `id` consistently for
relay-backed operations in that process.
If selected session references unknown policy, startup SHALL fail fast with
`validation_unknown_policy`.

#### Scenario: Resolve TUI startup from explicit session/bundle selectors

- **WHEN** operator starts TUI with `--bundle agentmux --session user`
- **AND** session `user` is configured in global TUI sessions
- **THEN** TUI resolves bundle `agentmux` and sender identity `user`

#### Scenario: Resolve TUI startup from global defaults

- **WHEN** operator starts TUI without `--bundle`/`--session`
- **AND** global `tui.toml` defines `default-bundle` and `default-session`
- **THEN** TUI resolves startup identity from those defaults

#### Scenario: Reject sender flag at startup

- **WHEN** operator starts TUI with `--sender relay`
- **THEN** startup fails as unknown argument

#### Scenario: Fail fast when required defaults are missing

- **WHEN** operator starts TUI without selectors
- **AND** required default keys are absent in global `tui.toml`
- **THEN** startup fails with stable validation code

#### Scenario: Reject default session with unknown policy

- **WHEN** operator starts TUI without selectors
- **AND** defaults resolve to session `user`
- **AND** session `user` references unknown policy
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

Same-bundle MVP lock SHALL remain enforced for transport and history updates.

When startup transport is unavailable, TUI SHALL attempt runtime relay
auto-start before rendering an unavailable state.

Auto-started relay lifecycle remains external in MVP; TUI exit SHALL NOT
auto-stop relay.

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

#### Scenario: Reject cross-bundle transport scope in MVP

- **WHEN** TUI transport scope attempts bundle outside associated context
- **THEN** request is rejected with `validation_cross_bundle_unsupported`

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
- `validation_cross_bundle_unsupported`
- `validation_invalid_params`
- `authorization_forbidden`

#### Scenario: Show deterministic validation error for unsupported target class

- **WHEN** relay returns `validation_invalid_params` for unsupported raww target
  class
- **THEN** TUI shows deterministic raww failure state and does not retry

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

