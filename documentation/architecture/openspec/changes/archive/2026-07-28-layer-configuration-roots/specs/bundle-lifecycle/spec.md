## MODIFIED Requirements

### Requirement: Dynamic Bundle File Watching

The relay host SHALL watch the effective bundles configuration for filesystem
changes when resolved relay configuration leaves `watch-bundles` absent or
`true`. The watcher SHALL observe the bundles directory of **every** configuration
layer, and SHALL reconcile against the **effective union** produced by the shared
effective-file lookup, in which an entry in an earlier layer shadows an entry of
the same bundle identifier in a later layer.

The watcher SHALL use a debounced reconcile-on-change model: on any debounced
notification the relay re-scans every layer, recomputes the effective union, and
reconciles the loaded bundle set against it. Debounce window SHALL be short
enough for interactive use (~200ms) and long enough to avoid acting on partial
writes.

Reconciliation SHALL be driven by change in the **effective** bundle set, not by
the physical file event:

- A new effective bundle: relay SHALL load and start the bundle runtime,
  equivalent to `bundle up`. Validation errors on the new file are recorded as
  startup failures; the relay continues serving other bundles unchanged.
- A file appearing in a layer that shadows a bundle currently supplied by a later
  layer SHALL be a **reload** of that bundle, not an unload followed by an
  unrelated load.
- A file being deleted while a file of the same identifier remains in a later
  layer SHALL be a **reload** onto the revealed definition, and SHALL NOT unload
  the bundle.
- An edit to a file that is currently shadowed by an entry in an earlier layer
  SHALL produce no effective change and SHALL NOT reload or disconnect sessions.
- A bundle disappearing from the effective union entirely: relay SHALL emit a
  typed error frame (`runtime_bundle_unloaded`) to every active session in that
  bundle, close all connections for that bundle, and unload the bundle from the
  runtime catalog. Principal store entries for the affected sessions SHALL be
  retained (the principal store is relay-level and not pruned on bundle unload).
- A modified effective definition: relay SHALL treat the change as a disappear
  followed by a new file: emit `runtime_bundle_reloaded`, disconnect all active
  sessions, then reload the bundle with the new configuration.

Reconciliation SHALL distinguish definitions by the physical file supplying them
as well as by content, so a file that is byte-identical to the one it shadows
still reloads. A bundle's relative path is the same in every layer — that is
what makes them the same identifier — so two layers may hold byte-identical
definitions differing only in which physical file supplies them. Content alone
therefore cannot tell a shadowing event from no event at all, and provenance
must participate in the comparison.

When resolved relay configuration sets `watch-bundles = false`, the relay SHALL
NOT start the bundle file watcher and SHALL ignore bundle file add/remove/modify
events in any layer until relay restart.

#### Scenario: New bundle file detected at runtime

- **WHEN** a new bundle TOML file appears in the bundles directory of a layer
- **AND** no earlier layer shadows that identifier
- **AND** the relay is running with watching enabled by `relay.toml` or defaults
- **THEN** relay loads and starts the new bundle without restart
- **AND** subsequent connections to that bundle succeed

#### Scenario: A file appearing in an earlier layer shadows a loaded bundle

- **WHEN** a bundle file appears in a layer earlier than the one currently
  supplying a loaded bundle of the same identifier
- **THEN** relay reloads that bundle from the earlier layer's definition
- **AND** emits `runtime_bundle_reloaded` rather than `runtime_bundle_unloaded`

#### Scenario: Deletion reveals a later layer's definition

- **WHEN** a bundle file is deleted from the layer currently supplying it
- **AND** a file of the same identifier exists in a later layer
- **THEN** relay reloads that bundle from the revealed definition
- **AND** the bundle is not unloaded

#### Scenario: Edit to a shadowed file is inert

- **WHEN** a bundle file is modified
- **AND** an entry of the same identifier in an earlier layer shadows it
- **THEN** the effective definition is unchanged
- **AND** no reload or session disconnection occurs

#### Scenario: Byte-identical shadowing still reloads

- **WHEN** a bundle file appears in an earlier layer whose content is identical
  to the definition it shadows
- **THEN** relay reloads that bundle from the earlier layer
- **AND** deleting it again reloads from the revealed definition

#### Scenario: Bundle removed from the effective union with active sessions

- **WHEN** a bundle's last remaining definition is removed from every layer
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_unloaded` to each active session before
  closing connections
- **AND** unloads the bundle from the runtime catalog

#### Scenario: Effective definition modified at runtime with active sessions

- **WHEN** the effective definition of a loaded bundle changes, whether by
  editing the file that currently supplies it in any layer
- **AND** one or more sessions are active in that bundle
- **THEN** relay emits `runtime_bundle_reloaded` to each active session before
  disconnect
- **AND** closes all connections
- **AND** reloads the bundle with the new configuration

#### Scenario: Watching disabled ignores every layer

- **WHEN** resolved relay configuration sets `watch-bundles = false`
- **THEN** no watcher is started for the bundles directory of any layer
