## 1. Remove the structural cause

- [x] 1.1 Move the `AGENTMUX_CONFIGURATION_DIRECTORY` name constant from its
  private definition in `src/runtime/paths.rs` into `src/configuration/types.rs`
  beside the other context variable names, and have the resolver in `paths.rs`
  read it from there. The comment in `types.rs` already states the invariant this
  restores; keep it accurate.
- [x] 1.2 Add the name to `INHERITED_CONTEXT_VARIABLE_NAMES`, so consumers that
  sanitize an inherited environment clear it with the other context variables.
- [x] 1.3 Add the configuration layer list to `BringUpContext` and to
  `BringUpContext::environment_entries`, and extend `VARIABLE_NAMES` to match.
  The existing test holding the two in agreement should fail if only one is
  extended — confirm it does before relying on it.

## 2. Normalize the layer list at root resolution

- [x] 2.1 Absolutize every layer of the effective configuration list in
  `resolve_configuration_roots`, for the command-line and environment tiers
  (the default tier is already absolute by construction). Use lexical
  absolutization against the process working directory, matching how the state
  root is normalized — not canonicalization, which would resolve symlinks and
  rewrite a declared layer into a path the operator never named.
- [x] 2.2 Confirm the relay-spawn path in `src/commands/tui.rs` still passes one
  `--configuration-directory` occurrence per layer and that the TUI and the relay
  it spawns resolve the same list when the TUI is launched from a relative path.

## 3. Stamp at configuration load

- [x] 3.1 Thread the resolved configuration layer list into bundle configuration
  load so it reaches the `BringUpContext` construction in
  `src/configuration/loaders.rs`. Carry the layer list itself, not a pre-joined
  string — see 3.3.
- [x] 3.2 Stamp it through the existing `stamp_context_environment` path, inside
  the `spawns_agent()` branch, so it is upsert-if-absent and coder-less members
  carry no entry. Do not add a spawn-time injector.
- [x] 3.3 Serialize the layer list only where the stamp is actually written.
  `environment_entries` currently returns every pair eagerly and
  `stamp_context_environment` then discards those whose name is already present,
  so joining there would evaluate the representation for members that never
  receive it and reject configurations 3.4 requires to load. Resolve the
  eagerness — test the name for absence before joining, or let the entry be
  produced fallibly — rather than accepting it. This is the one place the
  existing mechanism does not carry the new variable unchanged.
- [x] 3.4 Return a structured validation error, naming the offending layer, when
  serializing a list whose layer path contains the environment separator. Do not
  split the value and do not skip the stamp. A coder-less member is never
  stamped, and a coder-backed member declaring its own
  `AGENTMUX_CONFIGURATION_DIRECTORY` keeps that value and needs no
  serialization; both must still load.

## 4. Committed client configuration

- [x] 4.1 Remove `--configuration-directory` from the in-repo artifacts carrying
  an `agentmux host mcp` command line. Two of the three currently emit it;
  `codex/config.toml` does not.
- [x] 4.2 Extend `scripts/lint-client-configuration.sh` to reject
  `--configuration-directory` alongside `--state-directory`, with a message
  naming the consequence for the configuration root rather than reusing the
  rendezvous wording, which does not apply. Confirm the extended lint fails
  before 4.1 lands and passes after.

## 5. Documentation

- [ ] 5.1 Correct `src/runtime/README.md`, the subsystem architecture
  documentation for these tiers. Its Root Resolution section states that the
  repeatable flag is the escape hatch for a path containing the separator, which
  this change makes incomplete: the relay must serialize the list wherever a
  coder-backed member requires the stamp. The section also documents
  normalization for the state root only, and now needs the configuration-layer
  guarantee alongside it.
- [ ] 5.2 Correct `documentation/usage/maintainer-configuration-guide.md`, which
  documents the normalization asymmetry as intended — the passage stating that
  the configuration layer list is not normalized and that a relative layer
  resolves against the process working directory at lookup time. Check the other
  relative-path passages in that guide for the same claim.
- [ ] 5.3 Record the operator-facing consequences in
  `documentation/usage/operations.md`: a relative configuration directory is now
  absolutized at startup, and a member whose default configuration root does not
  exist reports missing configuration instead of being scaffolded.

## 6. Tests

- [x] 6.1 Cover the stamp: a coder-backed member declaring no
  `AGENTMUX_CONFIGURATION_DIRECTORY` receives the relay's layer list; one that
  declares it keeps its own value; a coder-less member receives no entry.
- [x] 6.2 Cover normalization. **Covered at the chokepoints rather than by a
  relay spawn, which is narrower than this task asked for.** Three facts compose
  to the end-to-end claim, and each is asserted where it can fail:
  `runtime_paths::a_relative_configuration_layer_normalizes_against_the_working_directory`
  (every layer is absolute after resolution, order preserved),
  `bring_up_context::stamps_the_relays_configuration_layer_list` (that resolved
  list is what reaches the member), and the pre-existing
  `runtime_paths::configuration_root_resolves_from_environment_when_no_flag`
  (a child with no flag reads it). A spawned relay would exercise the same three
  and add process plumbing.
  Teeth verified as this task required: removing the normalization call fails
  the first test. A companion,
  `runtime_paths::absolutizing_a_configuration_layer_does_not_resolve_symlinks`,
  guards the opposite mistake — it survives removing normalization but fails
  canonicalization, so the two are not redundant.
- [x] 6.3 Cover the hydration change: a member whose own default configuration
  root does not exist resolves the stamped root and scaffolds nothing at its
  default root. **No new test written, and the reason is checked rather than
  assumed.** `runtime_starter::refuses_to_scaffold_an_explicitly_named_root`
  already loops both non-default tiers, including `Environment`, asserting the
  fault *and* that the root is not created; that is the whole of the hydration
  behavior. What is new is only that a member now reaches the environment tier,
  which `bring_up_context::stamps_the_relays_configuration_layer_list` asserts.
  A third test would restate both.
- [x] 6.4 Cover the unrepresentable list in all three shapes: a layer path
  containing the separator produces the structured validation error naming that
  layer when a coder-backed member would be stamped; it does not block a bundle
  with only coder-less members; and it does not block a coder-backed member that
  declares its own `AGENTMUX_CONFIGURATION_DIRECTORY`, which keeps its value.
  The third case is what holds task 3.3, but only against the regression that
  actually matters: propagating the render's error before the presence check.
  Merely computing the value early and discarding it on the skip leaves all
  three passing, so a teeth check that only hoists the call proves nothing.
- [x] 6.5 Cover the sanitization leak: a harness clearing inherited context
  clears `AGENTMUX_CONFIGURATION_DIRECTORY`, and a suite run with that variable
  exported does not resolve against it.
