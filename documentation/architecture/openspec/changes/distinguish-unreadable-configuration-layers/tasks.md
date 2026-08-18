## 1. Error Surface

- [x] 1.1 Add `ConfigurationError::UnreadableConfigurationLayer { path, source }`
  and its `Display` arm, naming the physical path and the underlying cause
- [x] 1.2 Extend the conversions into `RuntimeError` and the relay error
  mapping so the variant survives the boundaries it already crosses

## 2. Fallible Lookup

- [x] 2.1 Replace `Path::is_file` in `supplied_configuration_path` with an
  `fs::metadata` probe classifying `NotFound` alone as absence and every other
  error — plus a path occupied by something other than a regular file — as the
  new fault; return `Result<Option<PathBuf>, ConfigurationError>`
- [x] 2.2 Make `effective_configuration_path` fallible, retaining its
  base-layer fallback for genuine absence
- [x] 2.3 Make `effective_bundle_definitions` fallible at each of the three
  points it reads the filesystem: `read_dir` faults on anything but `NotFound`,
  iterator items are no longer discarded by `flatten`, and a `.toml` entry that
  is not a regular file faults; a layer with no `bundles/` directory continues
  to contribute nothing and a non-`.toml` entry is still ignored whatever its
  type
- [x] 2.4 Thread the `Result` through the five per-artifact path helpers and
  `supplied_root_configuration_sources`

## 3. Consumer Policy

- [x] 3.1 Absorb the fallible signatures in the already-fallible callers:
  `configuration/loaders.rs`, `runtime/association.rs`,
  `relay/authorization/loading.rs`, `relay/authorization/resolution.rs`
- [x] 3.2 Fault in `runtime/starter.rs` bundle-discovery pre-flight rather than
  reading an unreadable layer as "no bundles defined"
- [x] 3.3 Render the fault as a finding in `commands/check.rs` without aborting
  the remaining report, and reflect it in the exit status
- [x] 3.4 Early-return from `relay/watcher.rs::reconcile_bundles` on a faulted
  enumeration, leaving catalog and reconcile state untouched

## 4. Coverage

- [x] 4.1 Unit coverage for the classification: absence for `NotFound`, fault
  for `PermissionDenied` and for `NotADirectory`, supplied for a readable file
- [x] 4.2 Integration coverage for an unreadable earlier layer with a valid
  later layer, asserting the fault names the earlier layer and that the later
  layer's value is not used, for a required and an optional artifact
- [x] 4.3 Integration coverage for an unreadable `bundles/` directory, and for a
  layer with no `bundles/` directory still resolving from a later layer
- [x] 4.4 Coverage that `reconcile_bundles` retains its catalog when enumeration
  faults, driven by making a layer unreadable rather than by injecting an error.
  Retention is asserted through the error code a connection receives while the
  layer is dark: a retained bundle fails on the unreadable layer, where a
  torn-down one would answer `validation_unknown_bundle`
- [x] 4.5 Verify each permission fixture actually bites before relying on it,
  and report a skip rather than passing vacuously when it does not. Probing the
  applied mode covers more than a `geteuid` check — root, `CAP_DAC_OVERRIDE`,
  and a filesystem that ignores modes all present the same way. No `unix` gate:
  the integration binary already uses Unix sockets unconditionally, so a cfg on
  one module would claim a portability the target does not have
- [x] 4.6 Coverage for a non-file occupying an artifact path and for a directory
  named `<identifier>.toml` under `bundles/`, both faulting rather than
  resolving from a later layer. These need no permission fixture, so they run
  everywhere the suite does, including as root
- [x] 4.7 Integration coverage for `check configuration`'s full policy against
  an unreadable layer: the finding is rendered, the checks after it still run,
  and the command fails. Plus the two places the policy is easy to get wrong —
  the suppressed no-bundles error, and quiet mode, where the finding line can be
  the only surviving record of the layer fault because a fail-fast check ends
  the run before the closing error carries it

## 5. Documentation

- [x] 5.1 Update `src/configuration/README.md` where it describes layer lookup
  as infallible
- [x] 5.2 Record the absence-versus-failure distinction in the
  `effective_configuration_path` doc comment, alongside the existing note on why
  per-file bespoke lookups were removed
