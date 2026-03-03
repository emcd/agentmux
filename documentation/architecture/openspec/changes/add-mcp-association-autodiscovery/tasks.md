## 1. Implementation

- [x] 1.1 Implement bundle auto-discovery using Git common-dir project basename.
- [x] 1.2 Implement sender auto-discovery using worktree root basename.
- [x] 1.3 Implement non-Git fallback discovery from current working-directory
      basename.
- [x] 1.4 Implement optional local override loading from
      `.auxiliary/configuration/tmuxmux/overrides/mcp.toml`.
- [x] 1.5 Standardize explicit MCP association flags to `--bundle-name` and
      `--session-name`.
- [x] 1.6 Implement deterministic precedence:
      CLI > local override file > auto-discovery.
- [x] 1.7 Validate discovered/overridden bundle exists in bundle configuration
      store and fail fast when unknown.
- [x] 1.8 Validate discovered/overridden sender session is a valid unique
      bundle member and fail fast when unknown or ambiguous.
- [x] 1.9 Return structured bootstrap errors for unknown bundle, unknown
      sender, and ambiguous sender.
- [ ] 1.10 Add tests for:
      clone discovery, worktree discovery, non-Git fallback, override
      precedence, unknown bundle failure, and ambiguous sender failure.
- [x] 1.11 Add Git ignore entry for
      `.auxiliary/configuration/tmuxmux/overrides/`.
- [x] 1.12 Add user-facing documentation for association precedence and
      override file usage.

## 2. Validation

- [x] 2.1 Run `cargo check --all-targets --all-features`.
- [x] 2.2 Run `cargo clippy --all-targets --all-features -- -D warnings`.
- [x] 2.3 Run `cargo test --all-features`.
