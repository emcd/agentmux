# Change: Add relay.toml configuration file

## Why

Relay-wide runtime controls currently live on `agentmux host relay` flags even
though they are durable operator policy, not one-off invocation behavior. The
relay also needs a stable config home for peer relay addresses before outbound
peer routing can be designed against a concrete schema.

## What Changes

- Add `<config-root>/relay.toml` as the relay-level configuration artifact.
- Add top-level `watch-bundles = false` as the durable bundle watcher opt-out,
  while retaining `agentmux host relay --no-watch` as a CLI override.
- Add top-level `require-session-credentials = true` as the durable Unix socket
  credential enforcement setting, while retaining
  `agentmux host relay --require-credentials` as a CLI override.
- Resolve relay settings with precedence:
  CLI override > environment override > `relay.toml` > defaults.
- Add a schema-only `[[peers]]` table whose required `address` field records
  outbound peer relay endpoints without initiating outbound routing yet.
- Keep peer PSK material out of `relay.toml`; peer credentials remain
  owner-only state artifacts under `<state-root>/peers/<peer-alias>.psk`.

## Impact

- Affected specs: `runtime-bootstrap`, `cli-surface`, `session-relay`,
  `relay-identity`
- Affected code: `src/relay/authorization/loading.rs`,
  `src/commands/host/arguments.rs`, `src/commands/host/help.rs`,
  `src/commands/mod.rs`, `src/commands/host/relay.rs`,
  `src/relay/connection.rs`, `src/relay/identity.rs`,
  `src/relay/watcher.rs`, `src/runtime/paths.rs`, `src/commands/check.rs`,
  starter configuration scaffolding, in-source documentation comments, and
  configuration validation tests
