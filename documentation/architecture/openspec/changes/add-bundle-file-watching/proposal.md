# Change: Add dynamic bundle file watching to relay host

## Why

The relay reads bundle configuration only at startup; adding, modifying, or
removing a bundle requires a full relay restart, which disconnects all active
sessions. A file watcher allows operators to manage bundle configuration at
runtime without restarts. Tracked as `todos/runtime/9`.

## What Changes

- Add `notify`-based file watcher to `agentmux host relay` (debounced
  reconcile-on-change; default: enabled).
- Two new typed error codes: `runtime_bundle_unloaded` (file removed) and
  `runtime_bundle_reloaded` (file modified/replaced). Distinct from
  `relay_unavailable` per D5 principle.
- New bundle file detected at runtime: load and start the bundle runtime,
  equivalent to `bundle up`. Validation failures on the new file are recorded;
  other bundles continue serving.
- Bundle file removed: **BREAKING** for active sessions — emit
  `runtime_bundle_unloaded` to all active sessions in that bundle, close
  connections, unload bundle. Principal store entries for those sessions are
  retained (relay-level store; no identity data is lost on bundle unload).
- Bundle file modified: treat as remove + add — emit `runtime_bundle_reloaded`,
  disconnect sessions, reload with new configuration.
- `--no-watch` CLI flag on `agentmux host relay` opts out of watching for the
  lifetime of that process. Migrates to `relay.toml` when that config file lands.

## Impact

- Affected specs: `session-relay`, `cli-surface`
- Affected code: `src/commands/host/relay.rs`, relay lifecycle, `Cargo.toml`
  (`notify` + debouncer dependency)
