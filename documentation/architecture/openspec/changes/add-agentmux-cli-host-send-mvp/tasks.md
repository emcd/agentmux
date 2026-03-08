## 1. Implementation

- [ ] 1.1 Add a new primary `agentmux` binary with subcommands:
      `host relay <bundle-id>`, `host mcp`, `list`, and `send`.
- [ ] 1.2 Route `host relay <bundle-id>` through existing relay runtime startup
      flow using shared runtime override flags.
- [ ] 1.3 Route `host mcp` through existing MCP startup flow using shared
      runtime override flags and sender association behavior.
- [ ] 1.4 Implement CLI `list` as a direct relay request client command.
- [ ] 1.5 Implement CLI `send` target-mode validation (`--target ...` xor
      `--broadcast`) and relay request client command.
- [ ] 1.6 Implement `send` message input resolution:
      `--message` first, else piped stdin, else structured missing-message
      validation error.
- [ ] 1.7 Keep `agentmux-relay` and `agentmux-mcp` as compatibility wrappers
      that delegate into shared command execution paths.
- [ ] 1.8 Update README/operator docs with canonical `agentmux host ...`,
      `agentmux list`, and `agentmux send` examples.

## 2. Testing

- [ ] 2.1 Add integration tests for command topology and argument parsing.
- [ ] 2.2 Add integration tests for `send` message-source precedence and
      missing-message failures.
- [ ] 2.3 Add integration tests confirming wrapper parity with new host
      subcommands.

## 3. Validation

- [ ] 3.1 Run `cargo check --all-targets --all-features`.
- [ ] 3.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] 3.3 Run `cargo test --all-features`.
