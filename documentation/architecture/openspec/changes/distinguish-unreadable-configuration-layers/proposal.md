## Why

Configuration layer lookup collapses permission and I/O failure into ordinary
absence. `supplied_configuration_path` tests candidates with `Path::is_file`,
which answers `false` for a file that exists but cannot be stat'd, and
`effective_bundle_definitions` discards every `read_dir` error. An existing but
unreadable earlier layer therefore contributes nothing and resolution continues
silently into later layers, producing an effective configuration the operator
did not author while reporting success.

The failure is worst where it is least visible: a higher-precedence layer exists
precisely to shadow a lower one, so the symptom of losing it is that the base
layer's value is used — which looks exactly like a correctly resolved single-root
deployment. Layer validation cannot catch this today, since it proves only that
each supplied element is a directory.

## What Changes

- Effective-file lookup becomes fallible and distinguishes `NotFound` (ordinary
  absence) from permission or I/O failure (a fault carrying the physical path
  and cause). Optional-artifact semantics are unchanged: absence of `mcp.toml`,
  `users.toml`, or `ui.toml` from every layer remains absence.
- Bundle-directory enumeration becomes fallible on the same distinction. A layer
  with no `bundles/` directory continues to contribute nothing, which is the
  common case; a `bundles/` directory that exists and cannot be read is a fault.
- Consumers select their own policy rather than inheriting a uniform fail-fast.
  Startup and configuration load fault; `check configuration` reports the
  unreadable layer as a finding; the relay watcher holds its last good
  reconciliation rather than terminating a running relay.
- **BREAKING** (internal API only): the `*_configuration_path` helpers and
  `effective_bundle_definitions` change signature from infallible to fallible.
  No CLI surface, configuration file format, or wire contract changes.

## Capabilities

### New Capabilities

None. This constrains behavior already governed by an existing requirement.

### Modified Capabilities

- `runtime-bootstrap`: the **XDG Configuration Root** requirement governs layer
  precedence, closedness, and absence semantics, but is silent on a layer that
  exists and cannot be read. It gains the rule that unreadability is a fault
  rather than absence, and scenarios covering an unreadable earlier layer, an
  unreadable bundles directory, and the consumer-policy split.

## Impact

- `src/configuration/paths.rs` — `supplied_configuration_path`,
  `effective_configuration_path`, `effective_bundle_definitions`,
  `supplied_root_configuration_sources`, and the five per-artifact path helpers.
- `src/configuration/loaders.rs`, `src/runtime/association.rs`,
  `src/relay/authorization/{loading,resolution}.rs` — already fallible; absorb a
  new error variant.
- `src/runtime/starter.rs:121` — bundle-discovery pre-flight; faults.
- `src/commands/check.rs:68,120,163` — reports rather than aborts, since an
  unreadable layer is the condition this command exists to surface.
- `src/relay/watcher.rs:204` — must not abort a live relay on a transient
  failure.
- No dependency changes. Test fixtures need a mode-0 directory, which constrains
  the coverage to Unix and to non-root test execution.
