## 1. Implementation

- [ ] 1.1 Implement manifest-preamble start rendering as first envelope line.
- [ ] 1.2 Implement RFC 822-style header rendering with required and optional
      fields.
- [ ] 1.3 Implement address rendering/parsing for
      `Display Name <session:session_name>`.
- [ ] 1.4 Implement MIME multipart rendering with deterministic boundary and
      part ordering.
- [ ] 1.5 Implement MIME closing boundary termination (`--<boundary>--`).
- [ ] 1.6 Implement canonical manifest preamble field rendering.
- [ ] 1.7 Serialize manifest preamble JSON as compact single-line output.
- [ ] 1.8 Implement required chat body part `text/plain; charset=utf-8`.
- [ ] 1.9 Implement parser validation rules for malformed envelope rejection.
- [ ] 1.10 Implement `Cc` informational handling separate from routing.
- [ ] 1.11 Reserve and document `application/vnd.tmuxmux.path-pointer+json`
      extension part for future use.
- [ ] 1.12 Implement token-budget batching and splitting across prompts with
      configurable `max_prompt_tokens` defaulting to `4096`.
- [ ] 1.13 Implement tokenizer-profile-based prompt token estimation for
      batching decisions.
- [ ] 1.14 Add conformance tests for valid envelopes, malformed edge cases,
      and token-budget split behavior.
- [ ] 1.15 Add user-facing documentation and examples for envelope format.

## 2. Validation

- [ ] 2.1 Run `cargo check --all-targets --all-features`.
- [ ] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 2.3 Run `cargo test --all-features`.
