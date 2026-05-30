## 1. Implementation

- [ ] 1.1 Add `notify` crate (with debouncer feature) to `Cargo.toml`.
- [ ] 1.2 Add `--no-watch` flag to the `agentmux host relay` subcommand in
      `src/commands/host/relay.rs`.
- [ ] 1.3 Implement `BundleWatcher` background task: spawned after initial
      bundle load completes, watching the bundles configuration directory (not
      the runtime state directory). Debounce window ~200ms. Shutdown cleanly
      when the relay host process exits.
- [ ] 1.4 Implement reconcile-on-change handler: on debounced notification,
      re-scan the full bundles directory and diff against the loaded bundle set.
      Three cases:
      (a) new file → validate config; on success load and start the bundle
          runtime (equiv. `bundle up`); on failure record and continue serving
          other bundles.
      (b) disappeared file → emit `runtime_bundle_unloaded` typed error to all
          active sessions for that bundle; close connections; unload from catalog.
      (c) modified file → full teardown (emit `runtime_bundle_reloaded`; close
          connections) then reload with new config (same as case a).
- [ ] 1.5 Add `runtime_bundle_unloaded` and `runtime_bundle_reloaded` typed
      error codes to the relay error contract.
- [ ] 1.6 Integration test: add a bundle TOML file at runtime → relay starts
      the bundle without restart; new connections to that bundle succeed.
- [ ] 1.7 Integration test: remove a bundle file at runtime → active sessions
      receive `runtime_bundle_unloaded` before disconnect; subsequent connection
      attempts to that bundle fail with `validation_unknown_bundle`.
- [ ] 1.8 Integration test: modify a bundle file at runtime → active sessions
      receive `runtime_bundle_reloaded` before disconnect; relay loads the new
      configuration.
- [ ] 1.9 Integration test: `--no-watch` → add or remove a bundle file at
      runtime; relay does not reconcile; restart required.
