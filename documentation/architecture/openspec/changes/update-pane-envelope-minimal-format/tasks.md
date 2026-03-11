## 1. Implementation

- [ ] 1.1 Update envelope renderer to remove injected JSON manifest preamble.
- [ ] 1.2 Update envelope renderer to omit:
      - `Envelope-Version`,
      - top-level multipart `Content-Type`,
      - per-part `Content-Transfer-Encoding`.
- [ ] 1.3 Preserve boundary start/end framing and required chat text part.
- [ ] 1.3 Preserve boundary start/end framing and enforce deterministic boundary
      token derivation from first boundary line.
- [ ] 1.4 Preserve human-addressing headers (`From`, `To`, optional `Cc`,
      optional `Subject`) plus `Message-Id` and `Date`.
- [ ] 1.5 Ensure machine metadata required for routing/audit is emitted via
      inscriptions/logs and not injected as pane envelope preamble, with
      required field parity (`schema_version`, `message_id`, `bundle_name`,
      `sender_session`, `target_sessions`, `created_at`).

## 2. Testing

- [ ] 2.1 Update envelope renderer tests to assert simplified envelope shape.
- [ ] 2.2 Update parser/validation tests to reject missing boundary/body and
      to stop requiring removed preamble/headers.
- [ ] 2.3 Add/adjust integration coverage to confirm delivery still succeeds
      with simplified envelopes.

## 3. Validation

- [ ] 3.1 Run `openspec validate update-pane-envelope-minimal-format --strict`.
- [ ] 3.2 Run `cargo check --all-targets --all-features`.
- [ ] 3.3 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 3.4 Run `cargo test --all-features`.
