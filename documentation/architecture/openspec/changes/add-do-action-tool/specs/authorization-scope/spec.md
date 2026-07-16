## ADDED Requirements

### Requirement: Configured Do Action Registry

The system SHALL support configured do-action entries for relay-dispatched
automation prompts.

Action entries SHALL be defined in `coders.toml` at canonical path
`[[coders.do-actions]]` for each coder so prompts can vary by
coder/session context.

Each action definition SHALL include:

- `id` (unique action key)
- `prompt` (template/prompt text to inject)
- optional `description`
- optional `self-only` (default `true`)

In alpha scope, `self-only` is a forward-compat policy field; non-self targeting is not
supported yet, so do-run behavior remains self-target-only regardless of this
field value.

Action definitions SHALL be resolved from active runtime configuration for the
sender/session context.

#### Scenario: Load configured action registry from canonical coder path

- **WHEN** `coders.toml` defines `[[coders]]` entries with nested
  `[[coders.do-actions]]` tables
- **THEN** relay resolves do-action entries from that canonical path

#### Scenario: Load configured action registry

- **WHEN** runtime configuration includes action definitions
- **THEN** relay resolves those actions for eligible sessions

#### Scenario: Reject duplicate action ids

- **WHEN** configuration contains duplicate action ids in one action set
- **THEN** system rejects configuration with validation error


### Requirement: Relay Do Safety and Execution Semantics

Relay do execution SHALL enforce:

- action allowlist from configuration
- self-target-only execution in alpha scope
- effective async behavior for self-target actions

alpha scope SHALL NOT introduce broader authorization constraints beyond `self-only`;
those are deferred to the existing authorization track.

Relay SHALL emit action lifecycle inscriptions for observability.

#### Scenario: Force async behavior for self run

- **WHEN** relay receives a valid do run request (self-target by alpha scope contract)
- **THEN** relay treats dispatch as accepted/queued
- **AND** does not block waiting for sync completion semantics

#### Scenario: Emit do lifecycle inscriptions

- **WHEN** relay processes do run request
- **THEN** relay emits inscriptions for request and downstream delivery
  lifecycle events
