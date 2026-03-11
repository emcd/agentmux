# Change: Simplify pane envelope surface format

## Why

Current injected envelopes include redundant machine-oriented metadata (JSON
preamble and MIME/email transport headers) that make prompts noisy and harder
for humans and agents to read. We want a leaner, LLM-facing envelope while
preserving machine metadata in inscriptions/logs.

## What Changes

- Remove JSON manifest preamble from injected pane text.
- Remove redundant transport-style headers from injected pane text:
  - `Envelope-Version`
  - top-level multipart `Content-Type`
  - per-part `Content-Transfer-Encoding`
- Keep boundary-delimited message framing and closing marker.
- Lock boundary token derivation without top-level multipart `Content-Type`
  (first boundary line introduces token; closing line must match).
- Keep human-relevant addressing headers (`From`, `To`, optional `Cc`,
  optional `Subject`) and timestamp/message identity headers.
- Clarify that canonical machine metadata remains out-of-band (relay
  inscriptions/logs), with explicit required field parity.
- Update malformed-envelope validation rules to match the simplified format.

## Impact

- Affected specs:
  - `pane-envelope`
- Affected code:
  - envelope renderer
  - envelope parser/validator
  - envelope-related tests
