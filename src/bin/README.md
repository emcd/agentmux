# Binary Entrypoints

This directory contains executable entrypoints.

## Files

- `agentmux.rs`
  - unified CLI executable.
  - delegates argument handling to `agentmux::commands::run_agentmux`.

## Notes

- `agentmux` is the canonical executable for host and operator workflows.
