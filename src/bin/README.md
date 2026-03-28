# Binary Entrypoints

This directory contains executable entrypoints.

## Files

- `agentmux.rs`
  - unified CLI executable for relay host, MCP host, lifecycle commands,
    operator commands, and TUI launch.
  - delegates argument handling to `agentmux::commands::run_agentmux`.

## Notes

- `agentmux` is the canonical executable for host and operator workflows.
- Runtime behavior and argument parsing are implemented in `src/commands/`.
