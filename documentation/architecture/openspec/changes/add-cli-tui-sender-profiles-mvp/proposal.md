# Change: Add Global TUI Sessions and Session-Selected CLI/TUI Identity (MVP)

## Why

Recent relay live-claim enforcement makes ad-hoc sender identity selection
fragile and unsafe for multi-client workflows. Operators need one durable
identity model for UI clients that is explicit and independent from agent
worktree session definitions, while staying aligned with canonical session
identity and policy contracts.

## What Changes

- Introduce global TUI session registry in `<config-root>/tui.toml`.
- Define `default-bundle` and `default-session` for TUI startup defaults.
- Define global TUI sessions in `[[sessions]]` with:
  - selector id
  - wire `session-id`
  - policy reference
- Keep `--bundle` as an override for TUI startup bundle selection.
- Use `--session` as the only sender selector for `send` and `tui` in MVP.
- Remove `--sender` from `send` and `tui` surfaces in MVP.
- Remove association-derived sender fallback for these surfaces.
- Keep relay identity model canonical:
  - stream/routing/auth principal remains `(bundle_name, session_id)`
- For `client_class=ui`, policy binding source is global TUI session `policy`
  reference resolved via policy definitions.
- Add an explicit operator migration note for `--sender` removal and session
  selection workflow.

## Impact

- Affected specs:
  - `cli-surface`
  - `runtime-bootstrap`
  - `tui-surface`
  - `session-relay`
- Affected code (implementation follow-up):
  - CLI argument parsing (`send`/`tui`)
  - runtime bootstrap session/default resolution
  - relay validation for UI session identity on stream and request paths
  - relay authorization lookup for UI sessions from global `tui.toml`
  - docs and integration tests
