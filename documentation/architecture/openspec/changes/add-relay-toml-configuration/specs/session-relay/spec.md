## MODIFIED Requirements

### Requirement: Dynamic Bundle File Watching

The relay host SHALL watch the bundles configuration directory for filesystem
changes when resolved relay configuration leaves `watch-bundles` absent or
`true`. The watcher SHALL use a debounced reconcile-on-change model: on any
debounced notification the relay re-scans the full directory and reconciles the
loaded bundle set against the on-disk set. Debounce window SHALL be short enough
for interactive use (~200ms) and long enough to avoid acting on partial writes.

On a new bundle file: relay SHALL load and start the bundle runtime, equivalent
to `bundle up`. Validation errors on the new file are recorded as startup
failures; the relay continues serving other bundles unchanged.

On a disappeared bundle file: relay SHALL emit a typed error frame
(`runtime_bundle_unloaded`) to every active session in that bundle, close all
connections for that bundle, and unload the bundle from the runtime catalog.
Principal store entries for the affected sessions SHALL be retained (the
principal store is relay-level and not pruned on bundle unload).

On a modified bundle file: relay SHALL treat the change as a disappear followed
by a new file: emit `runtime_bundle_reloaded`, disconnect all active sessions,
then reload the bundle with the new configuration.

When resolved relay configuration sets `watch-bundles = false`, the relay SHALL
NOT start the bundle file watcher and SHALL ignore bundle file add/remove/modify
events until relay restart.

#### Scenario: New bundle file detected at runtime

- **WHEN** a new bundle TOML file appears in the bundles directory
- **AND** the relay is running with watching enabled by `relay.toml` or defaults
- **THEN** relay loads and starts the new bundle without restart
- **AND** subsequent connections to that bundle succeed

#### Scenario: Bundle file removed at runtime with active sessions

- **WHEN** a bundle TOML file is removed from the bundles directory
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_unloaded` to each active session before
  disconnect
- **AND** closes all connections for that bundle
- **AND** unloads the bundle from the runtime catalog
- **AND** subsequent connection attempts for that bundle return
  `validation_unknown_bundle`

#### Scenario: Bundle file modified at runtime with active sessions

- **WHEN** a bundle TOML file is modified
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_reloaded` to each active session before
  disconnect
- **AND** closes all connections
- **AND** reloads the bundle with the new configuration

#### Scenario: Watch disabled by relay configuration

- **WHEN** relay configuration resolves `watch-bundles = false`
- **AND** a bundle file is added, removed, or modified at runtime
- **THEN** relay does NOT reconcile the bundle set
- **AND** changes take effect only after relay restart
