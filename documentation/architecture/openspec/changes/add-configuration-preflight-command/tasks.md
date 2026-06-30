## 1. Implementation

- [x] 1.1 Add `relay::preflight_bundle_configuration` (lifecycle + public
      re-export) reusing `load_bundle_configuration` + `load_authorization_context`
      with no tmux/runtime side effects.
- [x] 1.2 Add `src/commands/check.rs`: parse `configuration` subverb + optional
      `<bundle-id>` + runtime flags, discover bundles for the all-bundles case,
      fail-fast validation loop, and a details-preserving error mapper.
- [x] 1.3 Wire `check` into `src/commands/mod.rs` (router, `CheckArguments`,
      help text).
- [x] 1.4 Keep the command read-only (no `ensure_starter_configuration_layout`).

## 2. Tests

- [x] 2.1 Unit: `tests/unit/relay/preflight.rs` — valid bundle, unknown bundle,
      unknown-field detail (file path + offending field), invalid policy scope
      (proves the authorization layer is exercised).
- [x] 2.2 Integration: `tests/integration/cli/check.rs` — valid single/all
      bundles, unknown-field failure with detail, no-bundles failure, unknown
      subcommand.

## 3. Documentation

- [x] 3.1 Update `src/commands/README.md` and `src/relay/README.md` for the new
      command and relay entrypoint.
- [ ] 3.2 On archive, apply the cli-surface spec delta to
      `specs/cli-surface/spec.md`.
