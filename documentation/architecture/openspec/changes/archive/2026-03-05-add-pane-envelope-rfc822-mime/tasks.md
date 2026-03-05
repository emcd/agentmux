## 1. Implementation

- [x] 1.1 Implement manifest-preamble start rendering as first envelope line.
- [x] 1.2 Implement RFC 822-style header rendering with required and optional
      fields.
- [x] 1.3 Implement address rendering/parsing for
      `Display Name <session:session_name>`.
- [x] 1.4 Implement MIME multipart rendering with deterministic boundary and
      part ordering.
- [x] 1.5 Implement MIME closing boundary termination (`--<boundary>--`).
- [x] 1.6 Implement canonical manifest preamble field rendering.
- [x] 1.7 Serialize manifest preamble JSON as compact single-line output.
- [x] 1.8 Implement required chat body part `text/plain; charset=utf-8`.
- [x] 1.9 Implement parser validation rules for malformed envelope rejection.
- [x] 1.10 Implement `Cc` informational handling separate from routing.
- [x] 1.11 Reserve and document `application/vnd.tmuxmux.path-pointer+json`
      extension part for future use.
- [x] 1.12 Implement token-budget batching and splitting across prompts with
      configurable `max_prompt_tokens` defaulting to `4096`.
- [x] 1.13 Implement tokenizer-profile-based prompt token estimation for
      batching decisions.
- [x] 1.14 Add conformance tests for valid envelopes, malformed edge cases,
      and token-budget split behavior.
- [x] 1.15 Add user-facing documentation and examples for envelope format.

## 2. Validation

- [x] 2.1 Run `cargo check --all-targets --all-features`.
- [x] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 2.3 Run `cargo test --all-features`.
