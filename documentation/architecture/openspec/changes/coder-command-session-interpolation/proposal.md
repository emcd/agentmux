## Why

Agentmux needs to stamp its own session identity and worktree directory into
coder profile commands (e.g. Cistella `conduct` labels) so Agentmux/Cistella
correlation works through labels while each system keeps its own session
identity model. Today the only template variable is `{coder-session-id}`;
there is no way to reference the Agentmux session id or the member directory.
This change standardizes the vocabulary on double-brace names — migrating
that variable to `{{coder-session-id}}` — and adds the two missing ones.
This blocks the Cistella `revise-driver-surface` follow-on to
`add-cistella-driver` (`agentmux:todos/runtime/26`, milestone v0.10.0).

## What Changes

- Standardize coder profile command templates on one double-brace
  vocabulary: `{{coder-session-id}}` (migrated from `{coder-session-id}`),
  `{{bundle-session-id}}` (the Agentmux session id), and `{{session-directory}}`
  (the session's declared `directory`). Names are hyphenated per the existing
  `coder-session-id` convention, overriding the underscore spelling in
  `todos/runtime/26` per operator direction.
- Apply them in the existing per-session command rendering path
  (`render_command_template`), covering tmux/pty `initial-command` and
  `resume-command`.
- Extend unknown-placeholder detection to the double-brace vocabulary so an
  unresolved `{{name}}` fails configuration validation, per the existing
  transport-contracts rule that unknown placeholders fail load. The
  scanner detects both `{name}` and `{{name}}` shapes, but only the
  three double-brace names are accepted; every other occurrence —
  including the previously accepted `{coder-session-id}` form — is an
  unknown placeholder and fails load.
- Document the new variables in the maintainer configuration guide's
  `coders.toml` section.

## Capabilities

### New Capabilities

- (none — the interpolation mechanism already exists; this change extends its
  vocabulary)

### Modified Capabilities

- `transport-contracts`: the "Coder Command Template Resolution" requirement
  gains the `{{coder-session-id}}` (migrated), `{{bundle-session-id}}`, and
  `{{session-directory}}` placeholders, their value sources, and the
  unknown-placeholder rule over both detected shapes.

## Impact

- `src/configuration/targets.rs` (`render_command_template` and its callers);
  no changes to the `[[coders]]` schema, layer resolution, or spawn paths.
- **BREAKING**: the accepted vocabulary is three double-brace names only.
  The placeholder scanner detects both `{name}` and `{{name}}` shapes,
  but classification accepts only the three double-brace names and
  rejects every other occurrence as an unknown placeholder. This
  includes the previously accepted `{coder-session-id}` form: existing
  templates using the single-brace shape fail load with an "unknown
  placeholder" error and must be updated to the double-brace form. No
  compatibility path is preserved; the single-brace rejection is the
  migration mechanism itself. Templates containing underscore-bearing
  placeholder literals (e.g. `{{bundle_session_id}}`), which pass
  through silently today, likewise fail after this change.
- Open question for review (see design.md): the motivating example also uses
  `{{seat}}`, `{{bundle}}`, and `{{harness}}`, which do not exist in this
  repository and remain unknown placeholders after this change. This proposal
  is scoped to the two dispatched variables; the remaining three are a
  follow-on decision (add here vs. separate task).
