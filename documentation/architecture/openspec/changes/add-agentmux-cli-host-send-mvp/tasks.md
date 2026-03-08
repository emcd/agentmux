## 1. Implementation

- [x] 1.1 Add a new primary `agentmux` binary with subcommands:
      `host relay <bundle-id>`, `host mcp`, `list`, and `send`.
- [x] 1.2 Route `host relay <bundle-id>` through existing relay runtime startup
      flow using shared runtime override flags.
- [x] 1.3 Route `host mcp` through existing MCP startup flow using shared
      runtime override flags and sender association behavior.
- [x] 1.4 Implement CLI `list` as a direct relay request client command.
- [x] 1.5 Implement CLI `send` target-mode validation (`--target ...` xor
      `--broadcast`) and relay request client command.
- [x] 1.6 Implement `send` message input resolution:
      `--message` first, else piped stdin, else structured missing-message
      validation error.
- [x] 1.7 Keep `agentmux-relay` and `agentmux-mcp` as compatibility wrappers
      that delegate into shared command execution paths.
- [x] 1.8 Update README/operator docs with canonical `agentmux host ...`,
      `agentmux list`, and `agentmux send` examples.

## 2. Testing

- [x] 2.1 Add integration tests for command topology and argument parsing.
- [x] 2.2 Add integration tests for `send` message-source precedence and
      missing-message failures.
- [x] 2.3 Add integration tests confirming wrapper parity with new host
      subcommands.

## 3. Validation

- [x] 3.1 Run `cargo check --all-targets --all-features`.
- [x] 3.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 3.3 Run `cargo test --all-features`.
