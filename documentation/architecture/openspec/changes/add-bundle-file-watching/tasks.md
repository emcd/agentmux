## 1. Implementation

- [ ] 1.1 Add `notify` crate (with debouncer feature) to `Cargo.toml`.
- [ ] 1.2 Add `--no-watch` flag to the `agentmux host relay` subcommand in
      `src/commands/host/relay.rs`.
- [ ] 1.3 Convert `BundleCatalog` from immutable `Arc<HashMap>` to shared-
      mutable state (e.g., `Arc<RwLock<BundleCatalog>>`). Thread the write
      side through the relay host; connection handlers take short-lived read
      guards. This is the foundational change that enables dynamic reload;
      complete before implementing the watcher task.
- [ ] 1.4 Implement `BundleWatcher` background task: spawned after initial
      bundle load completes, watching the bundles configuration directory (not
      the runtime state directory). Debounce window ~200ms. Shutdown cleanly
      when the relay host process exits.
- [ ] 1.5 Implement reconcile-on-change handler: on debounced notification,
      re-scan the full bundles directory and diff against the loaded bundle set.
      Three cases:
      (a) new file → validate config; on success load and start the bundle
          runtime (equiv. `bundle up`); on failure record and continue serving
          other bundles.
      (b) disappeared file → evict all active sessions for that bundle via the
          shared eviction mechanism (task 1.6) with `runtime_bundle_unloaded`;
          unload from catalog.
      (c) modified file → diff the PARSED config against the running config;
          only evict and reload sessions whose definitions actually changed
          (comment/whitespace edits must not disconnect live agents); emit
          `runtime_bundle_reloaded` to affected sessions via task 1.6; reload
          changed bundle config.
- [ ] 1.6 Build shared session-eviction mechanism: a reusable
      `session_evict(session_id, typed_reason)` helper that emits the typed
      error frame to the target session and closes the connection. Used by
      file-watching (unloaded/reloaded) and identity Slice 2
      (revoked/expired). Single implementation; do not build independent
      per-feature eviction paths.
- [ ] 1.7 Add `runtime_bundle_unloaded` and `runtime_bundle_reloaded` typed
      error codes to the relay error contract.
- [ ] 1.8 Integration test: add a bundle TOML file at runtime → relay starts
      the bundle without restart; new connections to that bundle succeed.
- [ ] 1.9 Integration test: remove a bundle file at runtime → active sessions
      receive `runtime_bundle_unloaded` before disconnect; subsequent connection
      attempts to that bundle fail with `validation_unknown_bundle`.
- [ ] 1.10 Integration test: modify a bundle file at runtime → only sessions
      whose config definitions changed receive `runtime_bundle_reloaded`;
      sessions in unchanged definitions remain connected.
- [ ] 1.11 Integration test: modify a bundle file with whitespace/comment only
      → no sessions are disconnected.
- [ ] 1.12 Integration test: `--no-watch` → add or remove a bundle file at
      runtime; relay does not reconcile; restart required.
