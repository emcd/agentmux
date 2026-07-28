## 1. Layer list type and resolution

- [x] 1.1 Introduce a `ConfigurationRoots` value holding an ordered, non-empty
  layer list, constructed once during root resolution
- [x] 1.2 Accept `--configuration-directory` repeatably, appending one layer per
  occurrence in the order given
- [x] 1.3 Parse `AGENTMUX_CONFIGURATION_DIRECTORY` as a `:`-separated list in the
  same order, resolving relative elements against the working directory
- [x] 1.4 Make a supplied list closed: a file absent from every layer is absent,
  and no unsupplied root is consulted
- [x] 1.5 Resolve the XDG/home default as a single-layer list, so one code path
  serves every tier
- [x] 1.6 State the winning end of the list in the flag's help text and in the
  environment variable's documentation, rather than leaving it to be inferred

## 2. Effective-file lookup

- [x] 2.1 Generalize `effective_configuration_path` from first-of-two to
  first-of-N across the layer list
- [x] 2.2 Generalize `effective_bundle_definitions` to union N directories by
  identifier, earliest layer winning per identifier
- [x] 2.3 Remove the `overlay/` segment from every lookup, leaving a layer as an
  ordinary configuration root
- [x] 2.4 Confirm each path-valued field keeps its existing resolution base, with
  a test proving a field resolves identically regardless of supplying layer
- [x] 2.5 Keep starter hydration restricted to a defaulted list, which is a
  single layer. Do **not** give credential administration layer semantics: it
  writes session pre-shared keys under the state root, and routing it through a
  configuration layer would move credentials into a shared, layered, and
  potentially committed tree

## 3. Threading

- [x] 3.1 Replace `configuration_root: &Path` with the layer list across the 77
  signatures that take it, letting the compiler enumerate the call sites
- [x] 3.2 Confirm no loader reaches the filesystem by joining a file name onto a
  single layer, which would bypass the list exactly as it previously bypassed
  the overlay

## 4. Watcher

- [x] 4.1 Watch every supplied layer rather than a fixed pair
- [x] 4.2 Reconcile against the effective union: a definition appearing in an
  earlier layer shadows and reloads, its removal reveals and reloads the later
  layer, an edit to a shadowed file is inert, and only disappearance from the
  union unloads
- [x] 4.3 Confirm the existing supplying-layer fingerprint distinguishes
  byte-identical files across N layers, and extend it if it does not

## 5. Source introspection

- [x] 5.1 Report the physical file supplying each resolved artifact from
  `agentmux check configuration` as default output, so a shadowed copy is
  distinguishable from the copy in effect. Report only artifacts a layer
  actually supplies, and emit physical paths rather than the relative forms a
  layer list may carry
- [x] 5.2 Accept `-q`/`--quiet` on `check configuration`, suppressing source
  reporting, per-bundle confirmations, and the summary while leaving the exit
  code and any failure report
- [x] 5.3 Resolve and report sources before validation runs, so a fail-fast
  failure does not truncate the report on the run that most needs it
- [x] 5.4 Write sources to stdout and failures to stderr, flushing stdout before
  any failure. Without the flush a piped transcript can show the failure ahead
  of the report explaining it, since stdout is block-buffered when piped

## 6. Discovery removal

- [x] 6.1 Remove `--discover-local-configuration` and the ancestor walk
- [x] 6.2 Remove the discovered tier from root resolution and its
  `ConfigurationRootSource` variant
- [x] 6.3 Remove the discovery inscription and its stderr report, resolving
  `agentmux:issues/runtime/5` if that issue is still open

## 7. Documentation

- [x] 7.1 Update `src/configuration/README.md`: the layer list replaces the
  overlay, including the first-wins direction, the closed list and what
  closedness does not mean, and the rejection of empty layer elements
- [x] 7.2 Update `src/runtime/README.md` root-resolution section
- [x] 7.3 Write the maintainer guide section on configuration layout, with
  worked examples of a base plus an R&D layer, and the migration note that an
  `overlay/` subdirectory silently stops being consulted
- [x] 7.4 Document how to introspect which layer supplied each artifact, since
  that is the operator's only way to diagnose a shadowed file
- [x] 7.5 Sweep prose for `overlay` references that now mean something else

## 8. Verification

- [ ] 8.1 `cargo fmt`, `cargo clippy --all-targets -D warnings`, and the full
  nextest suite
- [ ] 8.2 `openspec validate --all --strict`
- [x] 8.3 Prove the ordering rule with a test asserting the first layer wins,
  and a second asserting a supplied list never reaches the XDG default
- [ ] 8.4 Prove credential administration is untouched by layering: a
  `--write-config` operation under a multi-layer list still writes under the
  state root and writes nothing into any configuration layer
- [x] 8.5 Prove introspection identifies the supplying layer for an artifact
  present in more than one, including when every copy is valid
- [ ] 8.6 Exercise the release binary for layer resolution and bundle union,
  since build-profile-invisible defects motivated the same step previously
