## REMOVED Requirements
### Requirement: Manifest-Preamble Start Marker

**Reason**: JSON preamble is redundant in LLM-facing pane text and adds noise.

**Migration**: Keep machine metadata in inscriptions/logs; do not inject JSON
preamble into pane envelopes.

### Requirement: Canonical Manifest Preamble Fields

**Reason**: Canonical machine metadata is no longer represented in pane text.

**Migration**: Preserve these fields in relay-side metadata/inscriptions, not
in injected envelope body text.

## MODIFIED Requirements
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

### Requirement: MIME Multipart Envelope

The envelope SHALL use boundary-delimited framing in pane text.
Envelope end SHALL be indicated by closing boundary:

- `--<boundary>--`

The renderer SHALL NOT emit top-level multipart `Content-Type` header in pane
text.

#### Scenario: Render boundary-delimited envelope without top-level content type

- **WHEN** relay renders envelope headers and body
- **THEN** envelope includes boundary start and closing marker
- **AND** top-level `Content-Type: multipart/mixed; boundary=...` is absent

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

#### Scenario: Preserve routing independent of Cc header

- **WHEN** envelope includes `Cc` header values
- **THEN** delivery routing remains derived from relay request targets

### Requirement: Malformed Envelope Rejection

The parser SHALL reject malformed envelopes when required headers, boundary,
closing boundary, or required text body part are missing or invalid.

#### Scenario: Reject missing boundary start marker

- **WHEN** parsed envelope lacks boundary-delimited body start marker
- **THEN** parser reports envelope as malformed

#### Scenario: Reject missing text body part

- **WHEN** parsed envelope lacks required `text/plain` body part
- **THEN** parser reports envelope as malformed

## ADDED Requirements
### Requirement: Out-Of-Band Machine Metadata

Canonical machine metadata for routing/audit SHALL be preserved out-of-band in
relay-managed metadata streams (for example inscriptions/logs) rather than
injected into pane envelope text.

#### Scenario: Preserve machine metadata without pane preamble

- **WHEN** relay emits an injected envelope
- **THEN** pane text excludes JSON manifest preamble
- **AND** equivalent machine metadata remains available out-of-band
