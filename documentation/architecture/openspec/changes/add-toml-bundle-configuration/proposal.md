# Change: Adopt TOML Bundle Configuration Model

## Why

Current bundle configuration uses one JSON file per bundle and inlines session
startup command strings. That repeats coder command patterns across sessions
and lacks first-class shared coder definitions.

A TOML-first model with reusable coder templates improves maintainability,
reduces duplication, and better supports operator-managed session resumption
workflows such as `codex resume <session-id>`.

## What Changes

- Replace per-bundle JSON files with TOML files under `bundles/`:
  - `bundles/<bundle-id>.toml`
- Add shared coder definitions in `coders.toml`.
- Use kebab-case TOML keys/tables with Serde mapping to Rust snake_case fields.
- Add top-level `format-version` for forward-compatible format evolution.
- Add `coders.toml` with `[[coders]]` template definitions:
  - `id`
  - `initial-command` template
  - `resume-command` template
- Add `bundles/<bundle-id>.toml` with:
  - `[[sessions]]` entries with:
  - `id`
  - `name`
  - optional `display-name`
  - `directory`
  - `coder`
  - optional `coder-session-id`
- Add optional coder-level prompt-readiness fields:
  - `prompt-regex`
  - `prompt-inspect-lines`
  - `prompt-idle-column`
- Define startup command resolution:
  - use `resume-command` when `coder-session-id` is set
  - otherwise use `initial-command`
  - fail validation if required placeholders are unresolved.
- Perform a TOML-only cutover now (no JSON backward-compatibility path).
- Resolve default configuration root as:
  - debug builds: repository-local `.auxiliary/configuration/tmuxmux/` when
    present
  - otherwise: `$XDG_CONFIG_HOME/tmuxmux` or `~/.config/tmuxmux`
  - explicit CLI/config override paths continue to take precedence.
- Resolve configuration artifacts from that root:
  - `coders.toml`
  - `bundles/<bundle-id>.toml`
  where `<bundle-id>` is derived from the bundle filename.

## Impact

- Affected specs:
  - `session-relay`
  - `runtime-bootstrap`
- Affected code:
  - `src/configuration.rs`
  - `src/relay.rs`
  - `src/runtime/association.rs`
  - `src/runtime/paths.rs`
  - integration and unit tests that write/read bundle config fixtures
  - `README.md`
