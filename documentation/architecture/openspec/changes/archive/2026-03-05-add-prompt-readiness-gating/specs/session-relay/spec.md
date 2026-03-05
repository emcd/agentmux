## ADDED Requirements

### Requirement: Prompt-Readiness Template Gating

The system SHALL support optional per-member prompt-readiness templates that
must match before relay injection.

A prompt-readiness template SHALL support:

- `prompt_regex` (required)
- `inspect_lines` (optional, defaults to a bounded tail window)

`prompt_regex` SHALL be evaluated against a multiline string built from the
inspected non-empty tail lines of pane output.

#### Scenario: Deliver when prompt-readiness template matches

- **WHEN** target member has a prompt-readiness template
- **AND** pane output is quiescent
- **AND** `prompt_regex` matches the inspected multiline tail text
- **THEN** relay injects the message

#### Scenario: Match prompt plus status with one multiline regex

- **WHEN** target member uses one regex that spans prompt and status lines
- **AND** pane output tail contains those lines in order
- **THEN** relay treats target as prompt-ready

#### Scenario: Time out when quiescent pane never becomes prompt-ready

- **WHEN** target member has a prompt-readiness template
- **AND** pane output reaches quiescence
- **AND** template matching conditions do not become true before delivery
  timeout
- **THEN** relay reports delivery timeout with prompt-readiness mismatch reason
- **AND** relay does not inject the message

### Requirement: Prompt-Readiness Template Validation

The system SHALL validate prompt-readiness template regex during bundle
configuration loading.

#### Scenario: Reject invalid prompt regex

- **WHEN** bundle configuration includes a malformed `prompt_regex`
- **THEN** bundle loading fails with a structured configuration validation
  error
