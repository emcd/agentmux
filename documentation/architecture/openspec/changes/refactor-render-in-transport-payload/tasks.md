## 1. Sequencing

- [ ] 1.1 Confirm the implementation branch includes the completed
      `refactor-transport-write-interface` end state before changing the payload
      shape.

## 2. Transport Contract

- [ ] 2.1 Replace `DeliveryEnvelope.rendered` with structured message fields in
      `src/transports/contract.rs`.
- [ ] 2.2 Add transport-safe address/attribution structs as needed, reusing
      `crate::envelope::AddressIdentity` only if it does not couple transports to
      relay internals.
- [ ] 2.3 Update `Transport::mailw` documentation to require transport-owned
      rendering for representation-specific outputs.
- [ ] 2.4 Keep `raww` unchanged as raw input and preserve FIFO batch-barrier
      semantics.

## 3. Relay Worker Payload Construction

- [ ] 3.1 Replace `render_task_envelope` with a structured payload builder that
      derives canonical sender, target, cc, display names, created timestamp,
      message body, choice deciders, quiescence hints, and authenticated identity.
- [ ] 3.2 Ensure relay-authored attribution remains immutable from the transport's
      perspective: transports consume fields but do not infer sender, cc, or
      authenticated identity.
- [ ] 3.3 Rename the inscription's `bundle_name` field to `namespace` and update
      `ManifestPreamble` accordingly; the structured payload's `namespace` field
      drives both the inscription and transport rendering.
- [ ] 3.4 Rename `ManifestPreamble.bundle_name` to `namespace` without a
      `schema_version` bump.
- [ ] 3.5 Remove the separate coder/UI envelope builder split where it exists only
      to compensate for rendered-vs-structured payload differences.

## 4. Transport Rendering

- [ ] 4.1 Update `TmuxTransport` to render structured delivery messages into
      pane-envelope text before paste.
- [ ] 4.2 Update `AcpTransport` to render structured delivery messages into
      pane-envelope text before token-budget grouping and turn submission.
- [ ] 4.3 Keep ACP batching based on rendered prompt text after render, preserving
      current `batch_envelope_groups` behavior and outcome fan-out.
- [ ] 4.4 Update `UiTransport` to build `UiIncomingMessage` directly from the
      structured payload.

## 5. Cleanup

- [ ] 5.1 Delete relay-side rendered-envelope helpers that no longer have callers.
- [ ] 5.2 Remove R1/interim comments from `DeliveryEnvelope`, `UiTransport`, and
      relay delivery worker code.
- [ ] 5.3 Update `src/relay/README.md` and relevant module comments to describe
      transport-owned rendering.

## 6. Tests and Validation

- [ ] 6.1 Add or update unit tests for structured payload construction and
      transport rendering boundaries.
- [ ] 6.2 Update Tmux and ACP delivery tests to prove rendered pane-envelope text
      remains unchanged from the caller's perspective.
- [ ] 6.3 Update UI transport tests to prove stream event payloads still carry
      body, sender, cc, and authenticated identity.
- [ ] 6.4 Run a final manual `cargo fmt --check` sweep before commit, in
      addition to per-commit hook enforcement.
- [ ] 6.5 Run a final manual `cargo clippy -- -D warnings` sweep before commit,
      in addition to per-commit hook enforcement.
- [ ] 6.6 Run targeted relay delivery and transport tests.
- [ ] 6.7 Run `cargo test` if targeted validation does not cover changed paths.
