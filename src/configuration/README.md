# Configuration Layout

This directory loads and validates operator configuration artifacts, then
normalizes them into runtime structures used by CLI, MCP, relay, and TUI code.

## Modules

- `mod.rs`
  - Public re-exports and shared private constants.
- `types.rs`
  - Public configuration structs and enums used by downstream runtime code.
- `errors.rs`
  - `ConfigurationError` and error formatting/source support.
- `paths.rs`
  - Helpers for resolving configuration artifact paths.
- `raw.rs`
  - Raw TOML serde shapes and internal validated coder target descriptors.
- `fields.rs`
  - Shared field normalization, id/group validation, and best-effort path
    canonicalization helpers.
- `targets.rs`
  - Session-shape selection and coder target validation/resolution.
- `loaders.rs`
  - Public load APIs and per-file parsing/validation orchestration.

## Invariants

- `crate::configuration::*` is the public import surface for callers.
- Raw TOML structs stay private to the configuration module.
- Loader behavior should preserve existing config schema and validation errors
  unless a spec or task explicitly changes them.
