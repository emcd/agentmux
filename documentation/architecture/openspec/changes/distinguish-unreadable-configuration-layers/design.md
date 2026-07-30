## Context

`supplied_configuration_path` selects the first layer supplying a relative path
by testing `candidate.is_file()`. `Path::is_file` answers `false` for every
error, so a candidate that exists but cannot be stat'd is reported as not
supplied and the search proceeds to the next layer. `effective_bundle_definitions`
does the same for directories: `let Ok(entries) = read_dir(&directory) else {
continue }` discards the distinction between "this layer has no `bundles/`",
which is ordinary, and "this layer's `bundles/` denied permission", which is not.

The consequence is not uniform across consumers, and one of them is severe.
`reconcile_bundles` (`src/relay/watcher.rs:197-212`) treats the enumeration
result as ground truth for what exists on disk and unloads every loaded bundle
absent from it:

```rust
let on_disk: HashSet<String> = effective_bundle_definitions(configuration_roots)
    .into_keys()
    .collect();
let loaded = catalog.loaded_bundle_names();
for bundle_name in loaded.difference(&on_disk) {
    unload_bundle(catalog, bundle_name, state);
}
```

An enumeration that returns empty because a directory became unreadable is
therefore indistinguishable from every bundle having been deleted, and the
watcher tears down the running relay's entire catalog. This is the strongest
argument for the change and it also constrains the fix: the watcher must not
respond to the newly-visible failure by terminating instead, or the outcome is
the same outage by a different route.

## Goals / Non-Goals

**Goals:**

- Distinguish "no layer supplies this" from "a layer could not be read" at the
  point of lookup, preserving the physical path and the underlying cause.
- Preserve optional-artifact semantics exactly: an artifact absent from every
  readable layer is still absent.
- Let each consumer choose its response, so the relay survives what startup
  refuses.

**Non-Goals:**

- Eager validation of layer readability at list construction. Considered and
  rejected below.
- Any change to layer precedence, closedness, the flag or environment surface,
  or configuration file formats.
- Reworking `ConfigurationError` more broadly, or converting other infallible
  configuration helpers.

## Decisions

### Probe with `fs::metadata` and classify by `io::ErrorKind`

`supplied_configuration_path` replaces `is_file()` with `fs::metadata(&candidate)`
and classifies:

- `Ok(metadata)` where `metadata.is_file()` — this layer supplies the file.
- `Ok(metadata)` otherwise — this layer does not supply it. See the residual
  below.
- `Err(NotFound)` — this layer does not supply it. Also covers a dangling
  symlink, matching today's behavior, since `metadata` follows links exactly as
  `is_file` does.
- `Err(NotADirectory)` — this layer does not supply it. A non-directory
  component cannot contain the file, and layer-level type errors are already
  caught by list validation. Stable since Rust 1.83; the crate requires 1.90.
- `Err(_)` — fault, carrying the candidate path and the `io::Error`.

Alternative considered: check `PermissionDenied` specifically and treat all other
errors as absence. Rejected — it inverts the safe default. The failure mode this
change exists to prevent is a lookup answering "absent" when it does not know,
and an unenumerated error kind would silently rejoin that class. Faulting on the
unknown case is the conservative direction, and every kind we can name as
genuinely meaning "not here" is named above.

### A distinct `ConfigurationError` variant rather than reusing `Io`

`ConfigurationError` already carries `Io { context: String, source: io::Error }`.
That variant is not sufficient here: `check configuration` and the watcher must
*match* on this condition to apply their policies, and matching on a formatted
context string is not a contract. Add:

```rust
UnreadableConfigurationLayer { path: PathBuf, source: io::Error },
```

`ConfigurationError` derives only `Debug`, so holding an `io::Error` is
consistent with the existing `Io` variant and costs no derive.

### Fallible signatures, infallible callers where they already are

- `supplied_configuration_path` → `Result<Option<PathBuf>, ConfigurationError>`.
  `Option` continues to mean supplied-or-not; `Result` carries the new axis.
- `effective_configuration_path` → `Result<PathBuf, ConfigurationError>`,
  retaining its base-layer fallback for the genuinely-absent case.
- `effective_bundle_definitions` → `Result<BTreeMap<String, PathBuf>, ConfigurationError>`,
  faulting on a `read_dir` error other than `NotFound`/`NotADirectory`.
- The five per-artifact helpers and `supplied_root_configuration_sources` inherit
  the `Result`.

Most callers already return `Result` with a `ConfigurationError` or a type that
converts from it (`src/configuration/loaders.rs`, `src/runtime/association.rs`,
`src/relay/authorization/{loading,resolution}.rs`), so the change is mechanical
there.

### Consumer policy is chosen per surface

The requirement the spec states is that the *distinction* is preserved. What a
surface does with a reported failure is that surface's decision:

- **Startup and configuration load** fault. `src/runtime/starter.rs:121` today
  asks whether any bundle definition exists; an unreadable layer must not answer
  "none" there.
- **`check configuration`** reports. It reads layers at
  `src/commands/check.rs:68,120,163`, and it is the surface an operator runs
  *because* something is wrong; aborting on the first unreadable layer withholds
  every other finding in the same run. It collects the fault as a finding and
  continues, and its exit status reflects that a finding was recorded.
- **The relay watcher** retains its last successful reconciliation. Per the
  Context above, both the current behavior and a naive fail-fast produce an
  outage. `reconcile_bundles` returns `()`; it gains an early return on a
  faulted enumeration, leaving `catalog` and `state` untouched so the next
  filesystem event reconciles normally once the layer is readable again.

Alternative considered: a single uniform fail-fast, as the issue's summary
phrases it. Rejected because it converts a recoverable configuration fault into
relay termination — strictly worse than the status quo for the one consumer
where the status quo is worst.

### Rejected: eager readability probe at layer-list construction

Validating readability once in `ConfigurationRoots::from_elements` would give a
single clear startup error and leave every lookup infallible. Rejected on two
grounds. It is TOCTOU-racy against the watcher, whose entire purpose is to
observe configuration changing under a running relay, so the check would pass at
boot and the defect would persist for exactly the consumer that suffers most
from it. And readability of a layer directory does not imply readability of the
artifacts beneath it, so the probe would license an assumption it cannot support.

## Risks / Trade-offs

- **Signature churn across 28 call sites in 8 files** → The change is mechanical
  where callers are already fallible, which is most of them. The three surfaces
  with genuine policy decisions are enumerated above and are the review focus.
- **Test fixtures require a mode-0 directory** → Coverage is Unix-only and
  cannot run as root, where the mode is not enforced. Gate the tests on
  `unix` and skip when `geteuid() == 0`, reporting the skip rather than passing
  vacuously. A test asserting a fault that silently never exercises the
  unreadable path is worse than no test.
- **Residual: a non-file at a layer's artifact path still reads as absence** →
  A directory named `coders.toml` in an earlier layer causes the same silent
  fallthrough this change closes for permission errors. It is deterministic and
  visible to an operator rather than invisible, so it is out of scope here, but
  it is the same defect class. See Open Questions.
- **A newly-surfaced fault may break a deployment that was silently tolerating
  an unreadable layer** → Acceptable and intended: such a deployment is already
  resolving configuration its operator did not author. Alpha defaults apply, so
  no compatibility shim.

## Migration Plan

No data or configuration migration. The change is internal-API-only; no CLI
surface, file format, or wire contract moves. Rollback is a revert.

## Open Questions

1. Should a non-file present at a layer's artifact path (a directory named
   `coders.toml`) fault rather than read as absence? It is the same silent-
   fallthrough class, but deterministic and operator-visible. Folding it in
   costs one match arm and one scenario; leaving it out keeps this change to the
   condition the issue names.
2. Should `check configuration`'s exit status distinguish an unreadable layer
   from other findings, or is "a finding was recorded" sufficient?
