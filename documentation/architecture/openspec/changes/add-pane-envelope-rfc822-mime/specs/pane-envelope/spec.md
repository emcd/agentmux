## ADDED Requirements

### Requirement: Manifest-Preamble Start Marker

Each injected envelope SHALL start with one compact JSON manifest preamble
line.

#### Scenario: Render manifest preamble as first line

- **WHEN** relay injects an envelope into a pane
- **THEN** the first non-empty line is compact JSON manifest preamble

### Requirement: RFC 822-Style Header Block

Envelope text after manifest preamble and before MIME body SHALL include RFC
822-style headers.

Required headers SHALL include:

- `Envelope-Version`
- `Message-Id`
- `Date`
- `From`
- `To`
- `Content-Type`

Optional headers MAY include:

- `Cc`
- `Subject`

#### Scenario: Render required headers

- **WHEN** relay renders an envelope
- **THEN** all required headers are present exactly once

#### Scenario: Accept envelope without subject header

- **WHEN** envelope omits `Subject`
- **THEN** envelope remains valid

### Requirement: Address Identity Format

Header addresses SHALL support display names and canonical session identifiers
using:

- `Display Name <session:session_name>`

#### Scenario: Render sender with display name

- **WHEN** sender display metadata is available
- **THEN** `From` header includes display name and `session:` identity token

### Requirement: MIME Multipart Envelope

The envelope SHALL use `multipart/mixed` with a valid boundary parameter.
Envelope end SHALL be indicated by MIME closing boundary:

- `--<boundary>--`

#### Scenario: Render multipart content type

- **WHEN** relay renders envelope headers
- **THEN** `Content-Type` is `multipart/mixed`
- **AND** includes a non-empty boundary value

#### Scenario: Render MIME closing boundary

- **WHEN** relay finishes rendering envelope MIME body
- **THEN** envelope terminates with `--<boundary>--`

### Requirement: Canonical Manifest Preamble Fields

Manifest preamble SHALL include:

- `schema_version`
- `message_id`
- `bundle_name`
- `sender_session`
- `target_sessions`
- `created_at`

Manifest MAY include:

- `cc_sessions`

Manifest serialization SHALL be compact single-line JSON.

#### Scenario: Include required preamble fields

- **WHEN** relay renders manifest preamble
- **THEN** all required preamble fields are present

#### Scenario: Render compact preamble JSON

- **WHEN** relay renders manifest preamble
- **THEN** preamble JSON is single-line compact serialization

### Requirement: Required Text Body Part

Envelope MIME parts SHALL include exactly one chat text part with
`Content-Type: text/plain; charset=utf-8`.

#### Scenario: Include chat body part

- **WHEN** relay renders envelope MIME parts
- **THEN** exactly one `text/plain` chat body part is present

### Requirement: CC Informational Semantics

`Cc` metadata SHALL be informational and SHALL NOT override canonical routing.

#### Scenario: Preserve routing from preamble targets

- **WHEN** envelope includes `Cc` header values
- **THEN** delivery routing remains derived from manifest preamble
  `target_sessions`

### Requirement: Extension Part Reservation

The system SHALL reserve MIME part type
`application/vnd.tmuxmux.path-pointer+json` for future pointer-style
attachments.

#### Scenario: Ignore reserved extension part in MVP

- **WHEN** parser encounters reserved path-pointer MIME part
- **THEN** parser ignores the part for MVP message execution
- **AND** does not treat presence as malformed

### Requirement: Prompt Batching Under Token Budget

The system SHALL support batching multiple envelopes into one injected prompt
when under configured token budget.

Default token budget SHALL be:

- `max_prompt_tokens = 4096`

The system SHALL estimate token count using configured tokenizer profile.

#### Scenario: Keep envelopes in one prompt under budget

- **WHEN** multiple envelopes together are at or below configured token budget
- **THEN** system injects them in one prompt preserving envelope order

#### Scenario: Split prompts when adding next envelope exceeds budget

- **WHEN** adding next envelope would exceed configured token budget
- **THEN** system starts a new prompt for that envelope
- **AND** preserves original envelope order across prompts

### Requirement: Malformed Envelope Rejection

The parser SHALL reject malformed envelopes when manifest preamble, required
headers, boundary, closing boundary, or required text body part are missing or
invalid.

#### Scenario: Reject missing manifest preamble

- **WHEN** parsed envelope lacks manifest preamble line
- **THEN** parser reports envelope as malformed

#### Scenario: Reject missing text body part

- **WHEN** parsed envelope lacks required `text/plain` body part
- **THEN** parser reports envelope as malformed
