# Development Guide

This guide is for contributors and coding agents working on `agentmux`.

End-user/operator material is documented under `documentation/usage/`.

## Local Validation

Default-features commands (run on every commit by the pre-commit hooks;
no Zig required):

```bash
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo nextest run --locked --config-file .auxiliary/configuration/nextest.toml
```

Pty-feature commands (run on Pty-source commits by the `cargo-clippy-pty`
pre-commit hook and on CI's `pty-feature` matrix entry; requires
Zig 0.15.x on `PATH` and outbound network for the libghostty-vt
vendored ghostty clone — or set `GHOSTTY_SOURCE_DIR` to a pre-checked-out
ghostty source tree):

```bash
cargo clippy --all-targets --features pty -- -D warnings
cargo nextest run --features pty --locked --config-file .auxiliary/configuration/nextest.toml
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
