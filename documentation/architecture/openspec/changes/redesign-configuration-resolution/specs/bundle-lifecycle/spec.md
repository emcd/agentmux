## MODIFIED Requirements

### Requirement: Dynamic Bundle File Watching

The relay host SHALL watch the effective bundles configuration for filesystem
changes when resolved relay configuration leaves `watch-bundles` absent or
`true`. The watcher SHALL observe **both** physical layers — the overlay bundles
directory and the base bundles directory — and SHALL reconcile against the
**effective union** produced by the shared effective-file lookup, in which an
overlay entry shadows a base entry of the same bundle identifier.

The watcher SHALL use a debounced reconcile-on-change model: on any debounced
notification the relay re-scans both layers, recomputes the effective union, and
reconciles the loaded bundle set against it. Debounce window SHALL be short
enough for interactive use (~200ms) and long enough to avoid acting on partial
writes.

Reconciliation SHALL be driven by change in the **effective** bundle set, not by
the physical file event:

- A new effective bundle: relay SHALL load and start the bundle runtime,
  equivalent to `bundle up`. Validation errors on the new file are recorded as
  startup failures; the relay continues serving other bundles unchanged.
- An overlay file appearing that shadows an existing base bundle SHALL be a
  **reload** of that bundle, not an unload followed by an unrelated load.
- An overlay file being deleted while a base file of the same identifier remains
  SHALL be a **reload** onto the revealed base definition, and SHALL NOT unload
  the bundle.
- An edit to a base file that is currently shadowed by an overlay entry SHALL
  produce no effective change and SHALL NOT reload or disconnect sessions.
- A bundle disappearing from the effective union entirely: relay SHALL emit a
  typed error frame (`runtime_bundle_unloaded`) to every active session in that
  bundle, close all connections for that bundle, and unload the bundle from the
  runtime catalog. Principal store entries for the affected sessions SHALL be
  retained (the principal store is relay-level and not pruned on bundle unload).
- A modified effective definition: relay SHALL treat the change as a disappear
  followed by a new file: emit `runtime_bundle_reloaded`, disconnect all active
  sessions, then reload the bundle with the new configuration.

When resolved relay configuration sets `watch-bundles = false`, the relay SHALL
NOT start the bundle file watcher and SHALL ignore bundle file add/remove/modify
events in either layer until relay restart.

#### Scenario: New bundle file detected at runtime

- **WHEN** a new bundle TOML file appears in the base bundles directory
- **AND** no overlay entry shadows that identifier
- **AND** the relay is running with watching enabled by `relay.toml` or defaults
- **THEN** relay loads and starts the new bundle without restart
- **AND** subsequent connections to that bundle succeed

#### Scenario: Overlay file appearing shadows a loaded base bundle

- **WHEN** an overlay bundle file appears whose identifier matches a loaded base
  bundle
- **THEN** relay reloads that bundle from the overlay definition
- **AND** emits `runtime_bundle_reloaded` rather than `runtime_bundle_unloaded`

#### Scenario: Overlay file deletion reveals the base definition

- **WHEN** an overlay bundle file is deleted
- **AND** a base bundle file of the same identifier exists
- **THEN** relay reloads that bundle from the revealed base definition
- **AND** the bundle is not unloaded

#### Scenario: Edit to a shadowed base file is inert

- **WHEN** a base bundle file is modified
- **AND** an overlay entry of the same identifier shadows it
- **THEN** the effective definition is unchanged
- **AND** no reload or session disconnection occurs

#### Scenario: Bundle removed from the effective union with active sessions

- **WHEN** a bundle's last remaining definition is removed from both layers
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_unloaded` to each active session before
  closing connections
- **AND** unloads the bundle from the runtime catalog

#### Scenario: Effective definition modified at runtime with active sessions

- **WHEN** the effective definition of a loaded bundle changes, whether by
  editing the file that currently supplies it in either layer
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_reloaded` to each active session before
  disconnect
- **AND** closes all connections
- **AND** reloads the bundle with the new configuration

#### Scenario: Watching disabled ignores both layers

- **WHEN** resolved relay configuration sets `watch-bundles = false`
- **THEN** no watcher is started for either the overlay or the base bundles
  directory
