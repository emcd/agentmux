## 1. Error Surface

- [ ] 1.1 Add `ConfigurationError::UnreadableConfigurationLayer { path, source }`
  and its `Display` arm, naming the physical path and the underlying cause
- [ ] 1.2 Extend the conversions into `RuntimeError` and the relay error
  mapping so the variant survives the boundaries it already crosses

## 2. Fallible Lookup

- [ ] 2.1 Replace `Path::is_file` in `supplied_configuration_path` with an
  `fs::metadata` probe classifying `NotFound` and `NotADirectory` as absence and
  every other error as the new fault; return
  `Result<Option<PathBuf>, ConfigurationError>`
- [ ] 2.2 Make `effective_configuration_path` fallible, retaining its
  base-layer fallback for genuine absence
- [ ] 2.3 Make `effective_bundle_definitions` fallible, faulting on a `read_dir`
  error other than `NotFound`/`NotADirectory` and continuing to contribute
  nothing for a layer with no `bundles/` directory
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

- [ ] 4.1 Unit coverage for the classification: absence for `NotFound` and
  `NotADirectory`, fault for `PermissionDenied`, supplied for a readable file
- [ ] 4.2 Integration coverage for an unreadable earlier layer with a valid
  later layer, asserting the fault names the earlier layer and that the later
  layer's value is not used, for a required and an optional artifact
- [ ] 4.3 Integration coverage for an unreadable `bundles/` directory, and for a
  layer with no `bundles/` directory still resolving from a later layer
- [ ] 4.4 Coverage that `reconcile_bundles` retains its catalog when enumeration
  faults, driven by making a layer unreadable rather than by injecting an error
- [ ] 4.5 Gate the permission fixtures on `unix` and skip when running as root,
  reporting the skip rather than passing vacuously

## 5. Documentation

- [ ] 5.1 Update `src/configuration/README.md` where it describes layer lookup
  as infallible
- [ ] 5.2 Record the absence-versus-failure distinction in the
  `effective_configuration_path` doc comment, alongside the existing note on why
  per-file bespoke lookups were removed
