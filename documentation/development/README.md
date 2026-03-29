# Development Guide

This guide is for contributors and coding agents working on `agentmux`.

End-user/operator material is documented under `documentation/usage/`.

## Local Validation

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

## Source Map

- [src/README.md](../../src/README.md)
- [src/bin/README.md](../../src/bin/README.md)
- [src/runtime/README.md](../../src/runtime/README.md)
- [src/mcp/README.md](../../src/mcp/README.md)
- OpenSpec specs:
  `documentation/architecture/openspec/specs/`

## Local Override Paths (Development)

These are primarily for local debug/testing workflows and should not be treated
as end-user defaults:

- MCP association override:
  `.auxiliary/configuration/agentmux/overrides/mcp.toml`
- TUI session override:
  `.auxiliary/configuration/agentmux/overrides/tui.toml`
