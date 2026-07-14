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

## Zig-free Pty Builds

`libghostty-vt-sys = 0.2.0` (the FFI crate pulled by `--features pty`)
honors three upstream escape hatches that skip the vendored Zig
build chain. Two are usable today; the third is gated behind a
small wrapper gap described below.

- `GHOSTTY_SOURCE_DIR=<path>` — point at a pre-checked-out ghostty
  source tree containing `build.zig`. The build script skips the
  `git clone` step; Zig is still required once to drive the build.
  Use when CI can install Zig but cannot reach `github.com` at build
  time (recommended for sandboxed CI / Nix).
- `GHOSTTY_ZIG_SYSTEM_DIR=<path>` — point at a pre-fetched Zig
  package cache (the output of a prior `zig build` against the same
  ghostty source). Combined with `GHOSTTY_SOURCE_DIR`, this skips
  both the network clone and the Zig package download. Use for fully
  air-gapped package-manager builds.
- `libghostty-vt-sys/pkg-config` feature — point at an installed
  `libghostty-vt` via pkg-config, skipping the vendored Zig build
  entirely. Currently unreachable through agentmux's consumer dep:
  `libghostty-vt = "=0.2.0"` does not re-export the `pkg-config`
  feature from `libghostty-vt-sys` (it re-exports `kitty-graphics`
  and `link-dynamic` but not `pkg-config`). To unlock, file an
  upstream PR against `github.com/uzaaft/libghostty-rs` adding
  `pkg-config = ["libghostty-vt-sys/pkg-config"]` to
  `libghostty-vt/Cargo.toml`'s `[features]` table, or apply a local
  `[patch.crates-io]` override.

The Pty-feature CI matrix entry in `.github/workflows/tester.yaml`
already installs Zig via `mlugg/setup-zig@v2`, so these overrides
are not needed for CI. They are intended for sandboxed or
air-gapped operators who cannot (or do not want to) install Zig on
their build host.

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
