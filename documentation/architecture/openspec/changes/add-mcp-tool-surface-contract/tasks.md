## 1. Implementation

- [x] 1.1 Add MCP tool registration for `list` and `chat`.
- [x] 1.2 Implement request schema validation for each tool and reject invalid
      inputs with structured error responses.
- [x] 1.3 Implement `list` response schema for potential recipient sessions.
- [x] 1.4 Include optional recipient `display_name` metadata in `list` output
      when configured.
- [x] 1.5 Implement `chat` target-mode validation so exactly one of
      `targets` or `broadcast=true` is accepted.
- [x] 1.6 Implement `chat` validation that `targets` must be a non-empty list.
- [x] 1.7 Implement sender identity inference from MCP server association for
      `chat` requests.
- [x] 1.8 Implement synchronous `chat` response schema with aggregate status and
      per-target `results[]` entries.
- [x] 1.9 Include `sender_session` and optional `sender_display_name` in chat
      responses.
- [x] 1.10 Implement stable error mapping for MVP error codes, including
      `validation_unknown_sender`.
- [ ] 1.11 Add contract tests for each tool, including invalid argument cases,
      target conflicts, empty target lists, and partial delivery outcomes.
- [ ] 1.12 Add contract tests for sender inference behavior and unknown-sender
      failure handling.
- [ ] 1.13 Add user-facing documentation for tool schemas and examples.
- [x] 1.14 Document that bundle configuration is manual/operator-managed for
      MVP and not mutated through MCP tools.

## 2. Validation

- [x] 2.1 Run `cargo check --all-targets --all-features`.
- [x] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 2.3 Run `cargo test --all-features`.
