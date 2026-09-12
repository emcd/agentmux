# Contributing To Agentmux

Use the [Development Guide](development/README.md) for contributor setup and
validation. [Development Practices](development-practices.md) records the
project-wide engineering conventions used by contributors and coding agents.

## Prerequisites

The pre-commit hooks and CI use [cargo-nextest](https://nexte.st/) as the test
runner. Install it before running the test suite:

```bash
cargo install cargo-nextest --locked
```

## Validation

Run the default-feature validation commands before submitting a change:

```bash
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo nextest run --locked --config-file .auxiliary/configuration/nextest.toml
```

Changes to the Pty transport also require its feature-specific validation,
including Zig 0.15.x and the prerequisites documented in the development
guide.
