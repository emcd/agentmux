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
- Bundle file modified: treat as remove + add — emit `runtime_bundle_reloaded`
  to all active sessions in that bundle, disconnect them, reload with new
  configuration. Smart reload (disconnecting only sessions whose definitions
  changed) is deferred as a follow-on.
- `--no-watch` CLI flag on `agentmux host relay` opts out of watching for the
  lifetime of that process. Migrates to `relay.toml` when that config file lands.

## Key Implementation Notes

- **BundleCatalog mutability**: The current `BundleCatalog` is an immutable
  `Arc<HashMap>` shared by reference across all connection handlers. Dynamic
  reload requires converting it to shared-mutable state (e.g.,
  `Arc<RwLock<BundleCatalog>>`). The watcher task holds the write side;
  connection handlers hold short-lived read guards. This is the true center of
  gravity of the change — address it in task 1.3 before building the watcher.
- **Modify = full teardown (v0.7.0)**: A modify event is handled as a disappear
  followed by a new file — evict all active sessions in the bundle with
  `runtime_bundle_reloaded`, then reload with the new configuration. No
  parsed-config diffing in this change; smart reload (disconnecting only
  sessions whose definitions actually changed) is deferred as a follow-on per
  `designs/relay/5`. See task 1.5c.
- **Shared session-eviction mechanism**: File watching (bundle unloaded/reloaded)
  and identity Slice 2 (credential revoked/expired) both require "emit typed
  error frame then close connection." Build one reusable
  `session_evict(typed_reason)` helper (task 1.6) rather than independent
  implementations that will diverge.

## Impact

- Affected specs: `session-relay`, `cli-surface`
- Affected code: `src/commands/host/relay.rs`, relay lifecycle, `Cargo.toml`
  (`notify` + debouncer dependency)
