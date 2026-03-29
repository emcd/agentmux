## Context

Bundle `[[sessions]]` are optimized for agent transports with coder/session
runtime descriptors. TUI clients are operator-facing and not tied to worktree
working directories. Requiring per-bundle TUI session entries is high-friction
and duplicates global operator identity concerns.

## Goals

- Use one global TUI session model across bundles.
- Keep TUI session selection compatible with canonical relay identity contracts.
- Keep sender/bundle resolution deterministic and fail-fast.
- Remove CLI/TUI sender inference and sender spoof surface.

## Non-Goals

- Replacing bundle `[[sessions]]` for agent transports.
- Moving bundle-specific topology into `tui.toml`.
- Changing relay authorization decision ownership.

## Decisions

- Global TUI session registry lives in `<config-root>/tui.toml`.
- Config keys use kebab-case and session entries use array-of-tables form:
  - `default-bundle`
  - `default-session`
  - `[[sessions]]`
- Each global TUI session defines:
  - `id` (selector only; not used as wire identity)
  - `session-id` (wire sender identity used for hello/routing/auth principal)
  - optional `name`
  - `policy` (policy reference)
  - `id` values must be unique within `tui.toml` (fail-fast on duplicates)
- Resolution for `agentmux tui`:
  1. explicit `--bundle`
  2. `default-bundle`
  3. fail-fast (`validation_unknown_bundle`)

  and

  1. explicit `--session`
  2. `default-session`
  3. fail-fast (`validation_unknown_session`)
- Resolution for `agentmux send` sender identity:
  1. explicit `--session`
  2. `default-session`
  3. fail-fast (`validation_unknown_session`)
- Bundle resolution for `agentmux send`:
  1. explicit `--bundle`
  2. `default-bundle`
  3. fail-fast (`validation_unknown_bundle`)
- `--sender` is removed from `send` and `tui` in MVP.
- Relay remains canonical for identity/policy evaluation:
  - principal remains `(bundle_name, session_id)`
  - for `client_class=ui`, validates `session_id` via global TUI sessions
  - for `client_class=ui`, evaluates authorization from global TUI session
    `policy` reference.

## Bootstrap Defaults

- Project bootstrap/init should generate a default `tui.toml` with one safe
  default session so first-run UX is functional without manual authoring.

## Validation and Errors

- `validation_unknown_session` for missing/unknown selected session.
- `validation_unknown_sender` when selected session resolves to invalid sender
  identity shape.
- `validation_unknown_policy` when selected session references an unknown
  policy.
- `validation_unknown_bundle` when startup bundle cannot resolve.
- malformed `tui.toml` remains bootstrap validation failure.

## Security/Authorization

- Session selection chooses UI identity; relay remains the sole authorization
  decision point.
- For UI requests, policy source is the selected global TUI session's `policy`
  reference resolved via policy definitions.

## Risks / Trade-offs

- Adds relay/runtime complexity for global TUI session lookup.
  Mitigation: narrow MVP schema and strict validation.
- Removes quick ad-hoc sender override.
  Mitigation: provide fast session creation/update guidance and generated
  defaults.
