## 1. Bring-up context substrate

- [x] 1.1 Add the context environment variable constants in `configuration`, so
  `runtime` consumes them without a `configuration` to `runtime` back-edge
- [x] 1.2 Stamp context onto each coder-backed member's merged environment at
  configuration load, upsert-if-absent, skipped for `ui`/`pubsub` members
- [x] 1.3 Shape the stamp as general context propagation rather than a fixed
  bundle/session pair, so the deferred runtime-instance work can extend it
- [x] 1.4 Add a shared test helper that strips inherited context from any
  spawned test child, applied before test-supplied entries, at every
  `host mcp` launch seam
- [x] 1.5 Loader tests: two members in different bundles sharing a default
  environment each carry their own context; operator-declared values preserved;
  coder-less members skipped
- [x] 1.6 Verify the suite is green both with a clean environment and with
  context set in the parent process

## 2. Configuration root resolution

- [x] 2.1 Rename `--config-directory` to `--configuration-directory` with no
  compatibility alias, updating every call site and test
- [x] 2.2 Implement the root ladder: CLI > `AGENTMUX_CONFIGURATION_DIRECTORY` >
  discovery > XDG/home, with explicit tiers replacing rather than extending
- [x] 2.3 Remove build-profile gating from configuration-root resolution
- [x] 2.4 Add the opt-in discovery flag, default off: nearest-ancestor walk for a
  directory that is itself a configuration root, canonicalizing paths, with no
  dependency on Git metadata or package manifests
- [x] 2.5 Report the selected root on a diagnostic channel when discovery runs
- [x] 2.6 Remove `--repository-root`'s configuration-root role, leaving its state
  and inscriptions roles to the deferred work
- [x] 2.7 Restrict starter hydration to defaulted roots; an explicitly supplied
  root that does not exist becomes a recorded fault rather than scaffolding
- [x] 2.8 Leave state and inscriptions root gating untouched, and record why in
  the code so it is not mistaken for an oversight

## 3. Overlay resolution

- [x] 3.1 Rename the `overrides/` directory to `overlay/`
- [x] 3.2 Implement the shared effective-file lookup over
  `[root/overlay/P, root/P]` selecting the first existing regular file
- [x] 3.3 Make a malformed overlay file a fault that does not fall through to the
  base file
- [x] 3.4 Union bundle definition directories by identifier, overlay shadowing
  base
- [x] 3.5 Preserve each path-valued field's existing resolution base, and add
  tests proving a field supplied by an overlay file resolves identically to the
  same field supplied by a base file. This change introduces no new resolution
  base and alters no existing one
- [x] 3.6 Re-anchor the association file and `users.toml` beneath the
  configuration root, removing their build-profile gating and their bespoke
  per-file lookup
- [x] 3.7 Make `ui.toml` overlay-aware, replacing its root-only behavior
- [x] 3.8 Remove the association file's configuration-root field and its
  circular role
- [x] 3.9 Route every relay, TUI, CLI, and preflight loader through the shared
  lookup
- [x] 3.10 Watch both physical layers and reconcile against the effective union:
  overlay creation shadowing a base bundle reloads, overlay deletion revealing a
  base reloads rather than unloads, an edit to a shadowed base file is inert, and
  only disappearance from the union unloads
- [x] 3.11 Confirm starter hydration writes only to the base root

## 4. Association resolution

- [ ] 4.1 Add `--default-bundle` in the default tier, below the injected
  environment
- [ ] 4.2 Reorder the bundle ladder: `--bundle` > injected environment > overlay
  > `--default-bundle`
- [ ] 4.3 Reorder the session ladder: `--session-name` > injected environment >
  overlay > working-directory match
- [ ] 4.4 Normalize blank injected values to absent at the point the environment
  is read, so every consumer sees one answer
- [ ] 4.5 Delete Git-derived bundle auto-discovery
- [ ] 4.6 Delete Git-derived session auto-discovery, promoting the
  working-directory match from fallback to primary
- [ ] 4.7 Record an unresolved association with its cause instead of failing

## 5. Startup fault tolerance

- [ ] 5.1 Introduce an explicit readiness state holding either a ready context or
  a retained startup fault
- [ ] 5.2 Convert root, configuration, association, and runtime security faults
  into retained faults after `host mcp` is identifiable. Relay reachability is
  excluded: it is evaluated per request at tool time
- [ ] 5.3 Defer `host mcp` argument validation, using no partially parsed values
  and never falling through from malformed higher-level intent
- [ ] 5.4 Keep protocol initialization, tool listing, schemas, and `help` green
  in every readiness state
- [ ] 5.5 Validate each tool request on its own terms before consulting the
  readiness guard
- [ ] 5.6 Surface the retained cause with actionable detail in tool errors
- [ ] 5.7 Snapshot the retained fault until process restart
- [ ] 5.8 Retain the MCP list synthetic home-bundle fallback unchanged, including
  when the relay is unreachable. It is a deliberate exception to
  `relay_unavailable`, distinct from a retained startup fault, and is now
  cross-referenced normatively from Relay Connectivity Handling
- [ ] 5.9 Keep relay reachability out of the retained fault set: a complete
  operational context is `Ready` even when the relay is unreachable, and
  reachability continues to surface per request as `relay_unavailable`
- [ ] 5.10 Delete the explicit-versus-implicit association classification, which
  exists only to choose between failing and starting unassociated
- [ ] 5.11 Invert the existing tests asserting that an unknown bundle fails
  startup

## 6. Git usage reduction, bounded by the retained state provenance

Association-driven Git usage is removed here. Git-derived *state* provenance is
deliberately retained, because the repository-local state and inscriptions
branches remain gated in this change and those branches are only reachable when
a repository root is resolved. Deleting the probe, the debug repository-root
helper, or the Git common-dir lookup would leave the repository root permanently
unresolved, silently collapsing repository-local state onto the XDG default and
producing exactly the coexistence failure this change defers. The full collapse
belongs with the runtime-instance work that replaces the provenance.

- [ ] 6.1 Remove only the association consumers of Git metadata, leaving the
  common-dir lookup and the debug repository-root helper in place
- [ ] 6.2 Retain the source-checkout probe and the package-manifest marker, since
  the host-relay repository-root fallback still consumes them
- [ ] 6.3 Narrow the workspace context to what remains: the working directory
  plus the Git-derived repository-root provenance feeding state and inscriptions
- [ ] 6.4 Drop the workspace-root argument only from call sites that thread it
  purely to locate override files, which now resolve under the configuration
  root
- [ ] 6.5 Verify the repository-local state and inscriptions branches still
  activate in a debug build from a source checkout, with a test that fails if the
  repository root resolves to `None`, and a second test proving a linked worktree
  still resolves the common-dir owner root so worktrees continue sharing one
  relay
- [ ] 6.6 Record in code why the remaining Git usage exists and what removes it,
  so it is not mistaken for an oversight and deleted opportunistically

## 7. Repository configuration posture

- [ ] 7.1 Commit the repository's Agentmux configuration directory
- [ ] 7.2 Ignore only the overlay directory beneath it
- [ ] 7.3 Update the R&D MCP invocation to use the discovery flag and
  `--default-bundle`
- [ ] 7.4 Request the corresponding upstream Copier template change emitting
  `--default-bundle`

## 8. Documentation and drift

- [ ] 8.1 Update subsystem READMEs covering root resolution, the overlay, and
  association precedence
- [ ] 8.2 Sweep documentation and non-normative prose for stale
  `--config-directory` and `overrides/` references. Normative spec text is
  carried by the deltas in this change, not by this sweep
- [ ] 8.3 Confirm no live spec still describes Git-derived association discovery
  or startup failure on an unknown bundle

## 9. Verification

- [ ] 9.1 `cargo fmt`, `cargo clippy --all-targets -D warnings`, and the full
  nextest suite
- [ ] 9.2 `openspec validate --all --strict`
- [ ] 9.3 Exercise the release binary directly for root resolution, overlay
  shadowing, and green startup, since several defects this change fixes were
  invisible to debug-build testing
