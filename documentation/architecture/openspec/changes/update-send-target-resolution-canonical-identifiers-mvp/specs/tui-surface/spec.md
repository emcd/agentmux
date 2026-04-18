## MODIFIED Requirements

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
