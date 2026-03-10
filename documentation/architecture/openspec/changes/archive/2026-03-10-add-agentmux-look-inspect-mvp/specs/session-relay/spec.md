## ADDED Requirements

### Requirement: Relay Look Operation

The system SHALL provide a relay-level read-only inspection operation:
`look`.

`look` request fields SHALL include:

- `requester_session` (required)
- `target_session` (required)
- `lines` (optional)
- `bundle_name` (optional/redundant when bundle is already bound by
  association/socket context)

#### Scenario: Resolve bundle from associated runtime context

- **WHEN** look request omits `bundle_name`
- **THEN** relay resolves bundle from associated runtime context

#### Scenario: Accept redundant matching bundle name

- **WHEN** look request includes `bundle_name` matching associated runtime
  context
- **THEN** relay accepts request and proceeds with the look operation

#### Scenario: Reject mismatched bundle name in MVP

- **WHEN** look request includes `bundle_name` that does not match
  associated runtime context
- **THEN** relay rejects request with `validation_cross_bundle_unsupported`

### Requirement: Look Capture Window Bounds

Look capture window SHALL use deterministic bounds:

- default `lines = 120`
- maximum `lines = 1000`
- valid range `1..=1000`

#### Scenario: Apply default line window

- **WHEN** look request omits `lines`
- **THEN** relay captures using default `lines = 120`

#### Scenario: Reject out-of-range line window

- **WHEN** look request includes `lines` outside `1..=1000`
- **THEN** relay rejects request with `validation_invalid_lines`

### Requirement: Look Response Contract

Successful relay look responses SHALL include:

- `schema_version`
- `bundle_name`
- `requester_session`
- `target_session`
- `captured_at`
- `snapshot_lines` (`string[]`)

`snapshot_lines` ordering SHALL be oldest-to-newest.

#### Scenario: Return canonical look payload

- **WHEN** look succeeds
- **THEN** relay returns canonical look response payload
- **AND** `snapshot_lines` contains ordered snapshot lines from oldest to
  newest
