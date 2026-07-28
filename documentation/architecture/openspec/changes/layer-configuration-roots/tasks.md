## 0. Sequencing gate

`redesign-configuration-resolution` rewrites most of the requirements this
change modifies. Its deltas apply at archive, so any MODIFIED delta authored
here before that point replaces a baseline which will no longer exist. The
deltas in this change were drafted from that change's delta text; they must be
re-verified against the live specs once it archives, before any code is written.

- [ ] 0.1 Confirm `redesign-configuration-resolution` is archived
- [ ] 0.2 Re-verify each MODIFIED requirement here against the post-archive live
  text, scenario by scenario, since a MODIFIED delta replaces the whole
  requirement and a dropped scenario is invisible to `--strict`
- [ ] 0.3 Draft the deferred deltas from the post-archive live text: every
  overlay-bearing requirement in `bundle-lifecycle` and
  `ui-surface-configuration`, and the ones in `runtime-bootstrap` and
  `cli-surface` this change does not yet cover. Enumerate them by grepping the
  live specs for overlay references rather than working from memory; the count
  at drafting time was roughly 30, 13, 8, and 8 references respectively
- [ ] 0.4 Re-review the completed delta set before implementation. The change is
  not implementable until 0.3 lands, and the delta set is the contract the
  implementation is held to

## 1. Layer list type and resolution

- [ ] 1.1 Introduce a `ConfigurationRoots` value holding an ordered, non-empty
  layer list, constructed once during root resolution
- [ ] 1.2 Accept `--configuration-directory` repeatably, appending one layer per
  occurrence in the order given
- [ ] 1.3 Parse `AGENTMUX_CONFIGURATION_DIRECTORY` as a `:`-separated list in the
  same order, resolving relative elements against the working directory
- [ ] 1.4 Make a supplied list closed: a file absent from every layer is absent,
  and no unsupplied root is consulted
- [ ] 1.5 Resolve the XDG/home default as a single-layer list, so one code path
  serves every tier
- [ ] 1.6 State the winning end of the list in the flag's help text and in the
  environment variable's documentation, rather than leaving it to be inferred

## 2. Effective-file lookup

- [ ] 2.1 Generalize `effective_configuration_path` from first-of-two to
  first-of-N across the layer list
- [ ] 2.2 Generalize `effective_bundle_definitions` to union N directories by
  identifier, earliest layer winning per identifier
- [ ] 2.3 Remove the `overlay/` segment from every lookup, leaving a layer as an
  ordinary configuration root
- [ ] 2.4 Confirm each path-valued field keeps its existing resolution base, with
  a test proving a field resolves identically regardless of supplying layer
- [ ] 2.5 Keep starter hydration restricted to a defaulted list, which is a
  single layer. Do **not** give credential administration layer semantics: it
  writes session pre-shared keys under the state root, and routing it through a
  configuration layer would move credentials into a shared, layered, and
  potentially committed tree

## 3. Threading

- [ ] 3.1 Replace `configuration_root: &Path` with the layer list across the 77
  signatures that take it, letting the compiler enumerate the call sites
- [ ] 3.2 Confirm no loader reaches the filesystem by joining a file name onto a
  single layer, which would bypass the list exactly as it previously bypassed
  the overlay

## 4. Watcher

- [ ] 4.1 Watch every supplied layer rather than a fixed pair
- [ ] 4.2 Reconcile against the effective union: a definition appearing in an
  earlier layer shadows and reloads, its removal reveals and reloads the later
  layer, an edit to a shadowed file is inert, and only disappearance from the
  union unloads
- [ ] 4.3 Confirm the existing supplying-layer fingerprint distinguishes
  byte-identical files across N layers, and extend it if it does not

## 5. Discovery removal

- [ ] 5.1 Remove `--discover-local-configuration` and the ancestor walk
- [ ] 5.2 Remove the discovered tier from root resolution and its
  `ConfigurationRootSource` variant
- [ ] 5.3 Remove the discovery inscription and its stderr report, resolving
  `agentmux:issues/runtime/5` if that issue is still open

## 6. Documentation

- [ ] 6.1 Update `src/configuration/README.md`: the layer list replaces the
  overlay, including the first-wins direction, the closed list and what
  closedness does not mean, and the rejection of empty layer elements
- [ ] 6.2 Update `src/runtime/README.md` root-resolution section
- [ ] 6.3 Write the maintainer guide section on configuration layout, with
  worked examples of a base plus an R&D layer, and the migration note that an
  `overlay/` subdirectory silently stops being consulted
- [ ] 6.4 Sweep prose for `overlay` references that now mean something else

## 7. Verification

- [ ] 7.1 `cargo fmt`, `cargo clippy --all-targets -D warnings`, and the full
  nextest suite
- [ ] 7.2 `openspec validate --all --strict`
- [ ] 7.3 Prove the ordering rule with a test asserting the first layer wins,
  and a second asserting a supplied list never reaches the XDG default
- [ ] 7.4 Prove credential administration is untouched by layering: a
  `--write-config` operation under a multi-layer list still writes under the
  state root and writes nothing into any configuration layer
- [ ] 7.5 Exercise the release binary for layer resolution and bundle union,
  since build-profile-invisible defects motivated the same step previously
