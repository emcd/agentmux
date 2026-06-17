# pane-envelope Specification

## Purpose
The pane-text envelope format for tmux-delivered messages. The spec governs the RFC 822-style header block (required `Message-Id`, `Date`, `From`, `To`; optional `Cc`, `Subject`), MIME multipart boundary framing without a top-level `Content-Type` header, the required `text/plain` chat body part, and prompt batching under a default 4096-token budget with order-preserving splits. Canonical machine metadata (`schema_version`, `message_id`, `bundle_name`, `sender_session`, `target_sessions`, `created_at`) is carried out-of-band by relay-managed metadata streams rather than pane preamble.
## Requirements
### Requirement: RFC 822-Style Header Block

Envelope text before the boundary-delimited body SHALL include RFC 822-style
headers focused on human-readable context.

Required headers SHALL include:

- `Message-Id`
- `Date`
- `From`
- `To`

Optional headers MAY include:

- `Cc`
- `Subject`

#### Scenario: Render required headers

- **WHEN** relay renders an envelope
- **THEN** all required headers are present exactly once
- **AND** removed transport headers are not rendered

#### Scenario: Accept envelope without subject header

- **WHEN** envelope omits `Subject`
- **THEN** envelope remains valid

### Requirement: Address Identity Format

Header addresses SHALL support display names and canonical session identifiers
using:

- `Display Name <session:session@namespace>`

The `session:` identity token SHALL carry the canonical `session@namespace`
principal id, so a recipient in any namespace can derive a reply address from
the envelope alone. When an address has no configured display name in the
delivery bundle (for example a co-recipient in another namespace), the display
portion SHALL fall back to the canonical id.

#### Scenario: Render sender with display name

- **WHEN** sender display metadata is available
- **THEN** `From` header includes display name and `session:` identity token
- **AND** the identity token carries the canonical `session@namespace` id

#### Scenario: Render cross-namespace address without configured display name

- **WHEN** an address has no configured display name in the delivery bundle
- **THEN** the address renders the canonical id in both the display and
  identity positions

### Requirement: MIME Multipart Envelope

The envelope SHALL use boundary-delimited framing in pane text.

Boundary token introduction SHALL be:

- first boundary line immediately after header block:
  - `--agentmux-<message-id-without-hyphens>`

Envelope end SHALL be indicated by matching closing boundary:

- `--<boundary>--`

The parser SHALL derive the boundary token from the first boundary line and
require the same token in the closing boundary line.

The renderer SHALL NOT emit top-level multipart `Content-Type` header in pane
text.

#### Scenario: Render boundary-delimited envelope without top-level content type

- **WHEN** relay renders envelope headers and body
- **THEN** envelope includes boundary start and closing marker
- **AND** top-level `Content-Type: multipart/mixed; boundary=...` is absent

#### Scenario: Reject closing boundary token mismatch

- **WHEN** parsed envelope closing boundary token differs from first boundary
  token
- **THEN** parser reports envelope as malformed

### Requirement: Required Text Body Part

Envelope parts SHALL include exactly one chat text part with:

- `Content-Type: text/plain; charset=utf-8`

The renderer SHALL NOT emit per-part `Content-Transfer-Encoding` header.

#### Scenario: Include chat body part without transfer-encoding header

- **WHEN** relay renders envelope body part
- **THEN** exactly one `text/plain` chat body part is present
- **AND** `Content-Transfer-Encoding` is absent

### Requirement: CC Informational Semantics

`Cc` metadata SHALL be informational and SHALL NOT override canonical routing.

Each delivered envelope's `Cc` header SHALL list every co-recipient of the
message — the full target set minus the envelope's own recipient — including
co-recipients in other namespaces.

#### Scenario: Preserve routing independent of Cc header

- **WHEN** envelope includes `Cc` header values
- **THEN** delivery routing remains derived from relay request targets

#### Scenario: Show cross-namespace co-recipients in Cc

- **WHEN** a message targets recipients in more than one namespace
- **THEN** each delivered envelope's `Cc` lists the co-recipients from every
  other namespace as canonical ids

### Requirement: Extension Part Reservation

The system SHALL reserve MIME part type
`application/vnd.agentmux.path-pointer+json` for future pointer-style
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

The parser SHALL reject malformed envelopes when required headers, boundary,
closing boundary, or required text body part are missing or invalid.

#### Scenario: Reject missing boundary start marker

- **WHEN** parsed envelope lacks boundary-delimited body start marker
- **THEN** parser reports envelope as malformed

#### Scenario: Reject missing text body part

- **WHEN** parsed envelope lacks required `text/plain` body part
- **THEN** parser reports envelope as malformed

### Requirement: Out-Of-Band Machine Metadata

Canonical machine metadata for routing/audit SHALL be preserved out-of-band in
relay-managed metadata streams (for example inscriptions/logs) rather than
injected into pane envelope text.

Required out-of-band metadata fields SHALL include:

- `schema_version`
- `message_id`
- `bundle_name`
- `sender_session`
- `target_sessions`
- `created_at`

Optional out-of-band metadata fields MAY include:

- `cc_sessions`

#### Scenario: Preserve machine metadata without pane preamble

- **WHEN** relay emits an injected envelope
- **THEN** pane text excludes JSON manifest preamble
- **AND** equivalent machine metadata remains available out-of-band

#### Scenario: Preserve canonical metadata field set out-of-band

- **WHEN** relay emits simplified pane envelope text
- **THEN** out-of-band metadata includes all required canonical fields

