# Change: Add configuration pre-flight CLI command

## Why

Configuration errors are only discovered when the relay attempts to start or
reconcile a bundle. A typo in a bundle file (the real incident: `codex-session-id`
instead of `coder-session-id`) hard-fails relay startup with no operator-facing
way to pre-flight the config first; the failure is buried in inscriptions or a
startup-failure record. Operators need a read-only command that validates
configuration the same way startup does, so a typo fails early and clearly
(issues/cli/6).

## What Changes

- Add `agentmux check configuration [<bundle-id>]`: a read-only pre-flight that
  validates one named bundle (or every discoverable bundle when omitted) through
  the exact loading path the relay uses at startup, exiting non-zero on the first
  invalid bundle with the offending file path plus field-level detail.
- Add a public `relay::preflight_bundle_configuration` entrypoint that reuses
  `load_bundle_configuration` + `load_authorization_context` with no tmux or
  runtime side effects, so the pre-flight covers exactly what a live startup
  would reject — including policy-control scopes, session→policy references,
  `relay.toml` choices range, and `users.toml` policy mappings, none of which the
  public configuration loaders reach on their own.
- The command never scaffolds or mutates configuration (read-only).

## Impact

- Affected specs: cli-surface
- Affected code: `src/commands/check.rs` (new), `src/commands/mod.rs` (router,
  `CheckArguments`, help), `src/relay/lifecycle.rs` and `src/relay/mod.rs`
  (`preflight_bundle_configuration`); configuration loading is unchanged.
- Tests: `tests/unit/relay/preflight.rs`, `tests/integration/cli/check.rs`.

Note: the implementation landed under the dispatched issue `issues/cli/6`; this
proposal ratifies the corresponding cli-surface contract addition.
