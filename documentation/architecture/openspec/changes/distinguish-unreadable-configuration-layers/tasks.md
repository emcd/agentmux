## 1. Error Surface

- [ ] 1.1 Add `ConfigurationError::UnreadableConfigurationLayer { path, source }`
  and its `Display` arm, naming the physical path and the underlying cause
- [ ] 1.2 Extend the conversions into `RuntimeError` and the relay error
  mapping so the variant survives the boundaries it already crosses

## 2. Fallible Lookup

- [ ] 2.1 Replace `Path::is_file` in `supplied_configuration_path` with an
  `fs::metadata` probe classifying `NotFound` alone as absence and every other
  error — plus a path occupied by something other than a regular file — as the
  new fault; return `Result<Option<PathBuf>, ConfigurationError>`
- [ ] 2.2 Make `effective_configuration_path` fallible, retaining its
  base-layer fallback for genuine absence
- [ ] 2.3 Make `effective_bundle_definitions` fallible at each of the three
  points it reads the filesystem: `read_dir` faults on anything but `NotFound`,
  iterator items are no longer discarded by `flatten`, and a `.toml` entry that
  is not a regular file faults; a layer with no `bundles/` directory continues
  to contribute nothing and a non-`.toml` entry is still ignored whatever its
  type
- [ ] 2.4 Thread the `Result` through the five per-artifact path helpers and
  `supplied_root_configuration_sources`

## 3. Consumer Policy

- [ ] 3.1 Absorb the fallible signatures in the already-fallible callers:
  `configuration/loaders.rs`, `runtime/association.rs`,
  `relay/authorization/loading.rs`, `relay/authorization/resolution.rs`
- [ ] 3.2 Fault in `runtime/starter.rs` bundle-discovery pre-flight rather than
  reading an unreadable layer as "no bundles defined"
- [ ] 3.3 Render the fault as a finding in `commands/check.rs` without aborting
  the remaining report, and reflect it in the exit status
- [ ] 3.4 Early-return from `relay/watcher.rs::reconcile_bundles` on a faulted
  enumeration, leaving catalog and reconcile state untouched

## 4. Coverage

- [ ] 4.1 Unit coverage for the classification: absence for `NotFound`, fault
  for `PermissionDenied` and for `NotADirectory`, supplied for a readable file
- [ ] 4.2 Integration coverage for an unreadable earlier layer with a valid
  later layer, asserting the fault names the earlier layer and that the later
  layer's value is not used, for a required and an optional artifact
- [ ] 4.3 Integration coverage for an unreadable `bundles/` directory, and for a
  layer with no `bundles/` directory still resolving from a later layer
- [ ] 4.4 Coverage that `reconcile_bundles` retains its catalog when enumeration
  faults, driven by making a layer unreadable rather than by injecting an error
- [ ] 4.5 Gate the permission fixtures on `unix` and skip when running as root,
  reporting the skip rather than passing vacuously
- [ ] 4.6 Coverage for a non-file occupying an artifact path and for a directory
  named `<identifier>.toml` under `bundles/`, both faulting rather than
  resolving from a later layer. These need no permission fixture, so they run
  everywhere the suite does, including as root
- [ ] 4.7 Integration coverage for `check configuration`'s full policy against
  an unreadable layer: the finding is rendered, the checks after it still run,
  and the command fails

## 5. Documentation

- [ ] 5.1 Update `src/configuration/README.md` where it describes layer lookup
  as infallible
- [ ] 5.2 Record the absence-versus-failure distinction in the
  `effective_configuration_path` doc comment, alongside the existing note on why
  per-file bespoke lookups were removed
