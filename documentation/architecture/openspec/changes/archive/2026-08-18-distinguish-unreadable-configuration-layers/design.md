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

### Probe with `fs::metadata`, and treat only `NotFound` as absence

`supplied_configuration_path` replaces `is_file()` with `fs::metadata(&candidate)`
and classifies:

- `Ok(metadata)` where `metadata.is_file()` — this layer supplies the file.
- `Err(NotFound)` — this layer does not supply it. Also covers a dangling
  symlink, matching today's behavior, since `metadata` follows links exactly as
  `is_file` does.
- Everything else — fault, carrying the candidate path and the `io::Error`.

Exactly one condition means "this layer does not supply the file": nothing is at
that path. Every other answer is a statement about a path that *is* occupied, and
under fallthrough all of them produce the same single symptom — a lower layer's
value takes effect while the operator believes the higher one is in force. That
symptom is the whole subject of this change, so the classification is drawn
around what the lookup can actually prove rather than around which errors feel
like configuration mistakes.

Two cases this sweeps in are worth naming, because they are the ones an
absence-leaning classification would keep:

- **`Ok(metadata)` where the path is not a regular file** — a directory named
  `coders.toml` in an earlier layer. Deterministic and visible to anyone who
  looks, but nothing prompts an operator to look: the deployment resolves
  configuration successfully from the layer beneath.
- **`Err(NotADirectory)`** — a path component that must be a directory is a
  regular file, such as a layer whose `bundles` is a file while
  `bundles/<name>.toml` is looked up. Layer-list validation does not cover this:
  it proves each supplied *layer root* is a directory, and says nothing about
  intermediate components of a relative artifact path beneath it.

Alternative considered: check `PermissionDenied` specifically and treat all other
errors as absence. Rejected — it inverts the safe default. The failure mode this
change exists to prevent is a lookup answering "absent" when it does not know,
and an unenumerated error kind would silently rejoin that class.

### Enumeration classifies at three points, not one

Bundle enumeration reaches the filesystem three times per layer, and each is its
own opportunity to convert failure into an empty result:

- **Opening the directory.** `read_dir` returning `NotFound` means the layer has
  no `bundles/`, which is ordinary for a layer overriding only root-level
  artifacts. Every other error faults, on the same terms as the lookup above.
- **Each iterator item.** `ReadDir` yields `Result<DirEntry>`, and the current
  `entries.flatten()` discards the `Err` arm — a per-entry failure mid-directory
  silently truncates that layer's contribution rather than the whole of it, which
  is the same defect in a form that is harder to see because enumeration still
  appears to succeed. Item errors fault.
- **Typing each entry.** The extension is tested first, so a non-`.toml` entry is
  skipped whatever its type and an ordinary subdirectory under `bundles/` stays
  ordinary. A `.toml` entry must then be a regular file; if it is not, or if its
  metadata cannot be read, that faults. `NotFound` here alone is skipped: it means
  the entry was removed between enumeration and the probe, and the watcher
  reconciles from the filesystem event that removal raises.

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

  The process exit status carries no room to say more: `src/bin/agentmux.rs`
  maps every failure to `1` and every success to `0`. What distinguishes an
  unreadable layer from the command's other findings is therefore the structured
  error code, which is the axis the command already reports on, rather than a
  new numeric status that would have to be invented across every subcommand to
  mean anything.

  An unreadable layer also disarms one existing report: enumeration that faults
  yields no bundle definitions, and the pre-existing "no bundle configurations
  found" error would then name the wrong problem and abort ahead of the finding
  that explains it. That error is suppressed when enumeration faulted — there is
  no evidence for it, only an absence of evidence.
- **The relay watcher** retains its last successful reconciliation. Per the
  Context above, both the current behavior and a naive fail-fast produce an
  outage. `reconcile_bundles` returns `()`; it gains an early return on a
  faulted enumeration, leaving `catalog` and `state` untouched so the next
  filesystem event reconciles normally once the layer is readable again.

  Retention is about the catalog, not about admission. A connection arriving
  while the layer is unreadable still has to read the bundle definition, which
  is inside the directory that cannot be traversed, so it fails — under the
  load-fault policy above, naming the layer. That failure is what makes
  retention observable rather than a weakening of it: a torn-down catalog would
  have forgotten the bundle and answered `validation_unknown_bundle`, the same
  answer it gives for a bundle the operator deleted. The distinction between
  "cannot read this right now" and "this does not exist" is the whole change,
  restated at the connection boundary.

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
- **The iterator-item arm has no test that drives it** → A `ReadDir` item error
  comes from a `readdir` call failing partway through a directory, which no
  portable fixture can arrange. The arm is covered by construction — removing
  `flatten` makes the `Err` unignorable at the type level — and deliberately
  gets no test, since one written against a condition that never occurs would
  pass whatever the arm did.
- **Test fixtures require a mode-0 directory** → Coverage is Unix-only and
  cannot run as root, where the mode is not enforced. Gate the tests on
  `unix` and skip when `geteuid() == 0`, reporting the skip rather than passing
  vacuously. A test asserting a fault that silently never exercises the
  unreadable path is worse than no test.
- **A newly-surfaced fault may break a deployment that was silently tolerating
  an unreadable layer** → Acceptable and intended: such a deployment is already
  resolving configuration its operator did not author. Alpha defaults apply, so
  no compatibility shim.
- **Faulting on a non-file artifact path widens what breaks** → A deployment
  parking a directory at an artifact path, or shadowing a name with a
  non-regular file, now faults where it previously fell through. This is the
  intended reach of the change and not a separate risk: the fallthrough it
  replaces is the same silent substitution, differing only in being reproducible
  once someone thinks to look.

## Migration Plan

No data or configuration migration. The change is internal-API-only; no CLI
surface, file format, or wire contract moves. Rollback is a revert.

## Open Questions

None.
