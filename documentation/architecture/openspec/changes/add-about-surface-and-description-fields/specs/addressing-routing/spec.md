## ADDED Requirements

### Requirement: Bundle and Session Description Fields

Bundle configuration SHALL support optional description metadata fields:

- bundle-level `description`
- session-level `description` on each `[[sessions]]` entry

Description normalization and validation SHALL be:

- trim leading and trailing whitespace before persistence/use
- reject values that are empty after trim with `validation_invalid_description`
- preserve internal newlines
- enforce maximum UTF-8 character length after trim:
  - bundle `description` <= 2048
  - session `description` <= 512

Description fields MAY be omitted.

#### Scenario: Load bundle with valid descriptions

- **WHEN** bundle and session descriptions are within limits after trim
- **THEN** configuration loads successfully

#### Scenario: Reject whitespace-only bundle description

- **WHEN** bundle `description` contains only whitespace characters
- **THEN** runtime rejects configuration with `validation_invalid_description`

#### Scenario: Reject over-length session description

- **WHEN** session `description` exceeds 512 UTF-8 characters after trim
- **THEN** runtime rejects configuration with `validation_invalid_description`

#### Scenario: Preserve internal newlines in description

- **WHEN** a valid description includes internal newline characters
- **THEN** runtime preserves internal newline content in normalized value
