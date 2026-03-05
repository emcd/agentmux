## Context

Current relay design requires strict machine-parsable messaging and human
readability in attached panes. A JSON-only payload is robust for parsing but
less ergonomic for human scan and addressing context.

This change introduces a structured text envelope model inspired by RFC 822
headers and MIME multipart payloads.

## Goals / Non-Goals

- Goals:
  - Provide human-readable message headers (`From`, `To`, `Cc`, `Date`,
    `Message-Id`).
  - Preserve canonical machine parsing through a required compact JSON manifest
    preamble.
  - Support future multipart expansion without breaking parsers.
- Non-Goals:
  - Full SMTP/MIME transport interoperability.
  - Binary attachment transport in MVP.
  - Replacing bundle routing semantics with header-derived routing.

## Decisions

- Decision: use compact JSON manifest preamble as envelope start marker.
  - Envelope starts with one compact JSON line containing required manifest
    fields.
  - Rationale: start detection stays machine-reliable without extra marker
    tokens.

- Decision: use RFC 822-style header block after manifest preamble and before
  MIME body.
  - Required headers:
    - `Envelope-Version`
    - `Message-Id`
    - `Date`
    - `From`
    - `To`
    - `Content-Type` (`multipart/mixed; boundary=...`)
  - Optional headers:
    - `Cc`
    - `Subject`
  - Rationale: familiar visual grammar for human readers.

- Decision: `Subject` is optional in MVP.
  - Consequence: envelopes remain valid without `Subject`.
  - Rationale: keeps token overhead lower while allowing thread hints when
    useful.

- Decision: addresses support display names with canonical session identity.
  - Syntax:
    - `Display Name <session:session_name>`
  - Rationale: display names improve readability while `session_name` stays
    stable for identity.

- Decision: `Cc` is informational.
  - Consequence: routing is derived from canonical manifest target lists, not
    from parsing `To`/`Cc` headers.
  - Rationale: avoids ambiguity between visible audience and authoritative
    delivery set.

- Decision: require canonical manifest preamble fields.
  - Manifest preamble includes:
    - `schema_version`
    - `message_id`
    - `bundle_name`
    - `sender_session`
    - `target_sessions[]`
    - `cc_sessions[]` (optional)
    - `created_at`
  - Serialization:
    - compact JSON (single-line, no pretty-print)
  - Rationale: strict machine parse remains canonical for automation,
    validation, and future tooling while controlling token overhead.

- Decision: require one body part for chat text.
  - MIME part type:
    - `text/plain; charset=utf-8`
  - Rationale: simplest interoperable representation for agent prompts.

- Decision: reserve extension part types.
  - Reserved type:
    - `application/vnd.tmuxmux.path-pointer+json`
  - Intended fields:
    - `label`
    - `local_path`
    - `media_type`
    - `sha256` (optional)
  - Rationale: future local pointer attachments without redesigning envelope
    grammar.

- Decision: envelope end marker is MIME closing boundary.
  - Format:
    - `--<boundary>--`
  - Rationale: MIME-native end framing avoids extra custom trailer markers.

- Decision: allow multi-envelope prompt batching under token budget.
  - Defaults:
    - `max_prompt_tokens = 4096`
  - Behavior:
    - estimate candidate prompt tokens using configured tokenizer profile
    - batch multiple envelopes when under budget
    - split into additional prompts when adding next envelope would exceed
      budget
  - Rationale: improves efficiency without overwhelming model attention.

## Parsing Rules

1. Locate compact JSON manifest preamble line.
2. Parse header block until first blank line.
3. Validate required headers and boundary.
4. Parse MIME parts until closing boundary `--<boundary>--`.
5. Require exactly one valid text body part.
6. Ignore unknown optional parts.

If any required element is missing or invalid, reject envelope as malformed.

## Risks / Trade-offs

- MIME parsing is more complex than raw JSON injection.
- Header/manifest duplication can drift if generator is buggy.
- Strict parser validation may reject partially corrupted but human-readable
  messages.

## Migration Plan

1. Implement envelope renderer with deterministic header and part ordering.
2. Implement parser and conformance tests for malformed envelope cases.
3. Update relay injection to emit this envelope format.
4. Document envelope examples and extension guidance.
