## 1. Configuration Schema

- [x] 1.1 Extend the relay configuration loader for root-level `watch-bundles`,
      root-level `require-session-credentials`, `[choices].pending-max`, and
      top-level `[[peers]]` entries.
- [x] 1.2 Validate `[[peers]]` entries fail-fast: `address` is required,
      non-empty, and unknown peer fields are rejected.
- [x] 1.3 Return one normalized relay runtime configuration object from the
      startup/pre-flight load path so callers do not parse `relay.toml`
      independently.
- [x] 1.4 Add environment overrides for `AGENTMUX_RELAY_WATCH_BUNDLES` and
      `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS`, accepting only `true` and
      `false` as boolean values and validating all other values fail-fast.
- [x] 1.5 Apply precedence CLI > environment > file > defaults when building the
      normalized relay runtime configuration. Consumers must not re-apply
      precedence or defaulting.

## 2. Relay Startup Integration

- [x] 2.1 Treat `watch_bundles` and `require_session_credentials` in
      `RelayHostArguments` as optional CLI overrides, not defaults. Update the
      field types in `src/commands/mod.rs`, parser initialization and flag arms
      in `src/commands/host/arguments.rs`, and all host-startup consumers in
      `src/commands/host/relay.rs` to use the normalized configuration.
- [x] 2.2 Thread `watch-bundles` into bundle watcher startup; `false` skips
      spawning the watcher for the process lifetime.
- [x] 2.3 Thread `require-session-credentials` into Hello credential
      verification as a relay-level setting.
- [x] 2.4 Keep `[[peers]]` inert during startup: parse and validate the entries
      but do not open outbound sockets or add routing targets.
- [x] 2.5 Keep peer relay PSK storage outside `relay.toml`, using
      `<state-root>/peers/<peer-alias>.psk` for raw credentials and the
      principal store for hashes.

## 3. CLI and Documentation

- [x] 3.1 Update `--no-watch` and `--require-credentials` help text to describe
      them as CLI overrides above environment, `relay.toml`, and defaults.
- [x] 3.2 Update README, subsystem README, and in-source documentation comment
      references from CLI flags to `relay.toml` keys.
- [x] 3.3 Ensure `agentmux check configuration` validates the expanded
      `relay.toml` schema through the same path as relay startup. Depends on
      task 1.3: check configuration must call the shared loader rather than
      parsing `relay.toml` independently.

## 4. Tests

- [x] 4.1 Add configuration-loader tests for defaults, explicit relay settings,
      missing `address`, empty `address`, and unknown peer fields.
- [x] 4.2 Add host argument tests proving migrated flags override config/env
      values rather than acting as defaults.
- [x] 4.3 Add relay startup/watch tests proving `watch-bundles = false` prevents
      runtime reconcile and the default still enables watching.
- [x] 4.4 Add Hello/auth tests proving `require-session-credentials = true`
      rejects socket-trust sessions and the default accepts them.
