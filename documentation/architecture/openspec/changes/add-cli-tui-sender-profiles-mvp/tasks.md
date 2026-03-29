## 1. Contract Design

- [x] 1.1 Add CLI session selector contract for `send` and `tui`.
- [x] 1.2 Remove direct `--sender` selector from `send` and `tui` in MVP.
- [x] 1.3 Lock deterministic bundle/session resolution for TUI startup with
      fail-fast defaults behavior.

## 2. Global TUI Session Schema Contract

- [x] 2.1 Add `<config-root>/tui.toml` global schema with kebab-case keys:
      `default-bundle`, `default-session`, and `[[sessions]]`.
- [x] 2.2 Lock `[[sessions]]` fields: `id`, optional `name`,
      required `policy`.
- [x] 2.3 Lock error taxonomy for unknown bundle/session/sender/policy and
      malformed config.
- [x] 2.4 Lock session selector `id` uniqueness in `tui.toml` (fail-fast on
      duplicates).

## 3. Relay Contract Updates

- [x] 3.1 Keep relay hello/routing/auth principal canonical as
      `(bundle_name, session_id)` for `client_class=ui`.
- [x] 3.2 Lock relay UI authorization policy source to global TUI session
      `policy` references resolved via policy definitions.

## 4. Implementation Follow-up (post-approval)

- [x] 4.1 Implement CLI parsing/validation for `--session` and remove
      `--sender` on `send`/`tui`.
- [x] 4.2 Implement runtime bootstrap resolver for `default-bundle` and
      `default-session` from global `tui.toml`.
- [x] 4.3 Implement deterministic session selector resolution for `send`/`tui`
      and relay validation for UI sender/policy from global TUI sessions.
- [x] 4.4 Add integration tests for default resolution, unknown session,
      unknown bundle, unknown sender, unknown policy, and
      auth-deny passthrough.
- [x] 4.5 Add bootstrap-generated default `tui.toml` session and operator docs.
- [x] 4.6 Add explicit migration docs for `--sender` removal on `send`/`tui`
      and session-based replacements.

## 5. Validation

- [x] 5.1 Run `openspec validate add-cli-tui-sender-profiles-mvp --strict`.
