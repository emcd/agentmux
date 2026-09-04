## 1. Operator vocabulary

- [x] 1.1 Separate the actions that carry operator input — the variants holding
      the typed character — from those that do not, so the configurable subset
      is derived from the vocabulary rather than maintained as a hand-kept list
      that a later variant can fall out of.
- [x] 1.2 Give each action in that subset an operator-facing kebab-case name,
      and a lookup from that name back to the action.
- [x] 1.3 Give each `BindingContext` an operator-facing kebab-case name and its
      reverse lookup.
- [x] 1.4 Add a test asserting every action in the configurable subset and every
      `BindingContext` has a name and that no two share one, so a variant added
      later cannot be silently unnameable.
- [x] 1.5 Add a test that every data-carrying action is outside the configurable
      subset, so a variant added later carrying operator input cannot become
      nameable by default.
- [x] 1.6 Document the two vocabularies in `src/tui/actions/README.md` as the
      operator-facing surface, distinct from the internal identifiers, including
      why the data-carrying actions are outside it.

## 2. Chord grammar

- [x] 2.1 Implement a parser from the written chord form to `Chord::Key`,
      accepting the modifier spellings `Chord::display` emits.
- [x] 2.2 Reject the chord shapes that are not operator-facing — the
      handler-reproduction and typing shapes — with an error naming what was
      attempted.
- [x] 2.3 Add the round-trip test over the whole default table: every chord the
      help overlay presents that denotes a keystroke parses back to the chord
      that was printed, and the placeholder standing for typing does not parse.
- [x] 2.4 Teeth-check 2.3 by changing one `Chord::display` arm to emit a form
      the parser does not accept, and confirm the test fails.
- [x] 2.5 Add the symbolic modifier to the grammar, parsing to a modifier that
      is unresolved until the effective table is built.
- [x] 2.6 Resolve the symbolic modifier to `Ctrl` off macOS unconditionally, and
      on macOS to the configured selection, defaulting to `Ctrl`. Add a test per
      platform arm so neither resolution can be changed without a failure.

## 3. Configuration loading

- [ ] 3.1 Add the raw TOML shapes for the `[bindings]` group to
      `src/configuration/raw.rs`, keeping them private to the module.
- [ ] 3.2 Extend `UiConfiguration` with the validated binding group, the preset
      selection, and the macOS primary-modifier selection.
- [ ] 3.3 Parse and validate the group in the `ui.toml` loader, leaving the
      file's existing resolution, absence, and malformed-file behavior
      untouched.
- [ ] 3.4 Add tests for an absent group, an absent `ui.toml`, a group naming an
      unknown action, an unknown context, an unknown preset, and an unparseable
      chord.
- [ ] 3.5 Add a test that an invalid binding group applies no binding from that
      configuration, rather than the rows preceding the invalid one.
- [ ] 3.6 Reject an unrecognized key at any level of the group, and a value
      outside the permitted set for `primary-modifier-on-macos`, rather than
      ignoring either.
- [ ] 3.8 Derive each context's permitted action set from its compiled rows, and
      reject a configured or preset row binding an action the context does not
      declare, with an error naming the action and the context.
- [ ] 3.9 Add a test that binding a contextually inert action is rejected, using
      an action whose effect is guarded on another focused field, and a test
      that a new chord for an action the context already declares is accepted.
- [ ] 3.7 Add a fixture test that loads the configuration shape the specification
      documents verbatim and asserts the effective table it produces, so the
      published shape cannot drift from what the loader accepts.

## 4. Effective binding table

- [ ] 4.1 Build the effective table from the compiled default rows, the applied
      presets, the configured rows, and the probe outcome, resolving the
      symbolic modifier for the running platform as it builds.
- [ ] 4.2 Order configured rows ahead of preset rows and preset rows ahead of
      compiled rows within a context, and leave `binding_lookup_order`
      unchanged.
- [ ] 4.3 Implement explicit unbinding, so a chord configured against no action
      is inert rather than falling through to its preset or compiled default.
- [ ] 4.4 Add tests for row-level merge: an unnamed default survives, a
      configured row wins over the compiled row it shadows, a configured row
      wins over a preset row, and a configured contextual row does not shadow a
      compiled global row.
- [ ] 4.5 Add tests for the capability columns: a single value applies to both
      classes, a class-qualified value applies to its class, and an omitted
      class keeps its compiled default.
- [ ] 4.6 Teeth-check 4.2 by ordering configured rows after compiled rows and
      confirming the test that covers a configured chord shadowed by a broader
      compiled row fails.

## 5. Wiring the consumers

- [ ] 5.1 Hold the effective table on the workbench, built where the loaded
      configuration and the probe outcome are both in hand.
- [ ] 5.2 Resolve dispatch against the effective table.
- [ ] 5.3 Generate the help overlay and the pane hint strips from the effective
      table.
- [ ] 5.4 Keep `examples/tui-binding-reference.rs` and the usage guide reading
      the default table, and state in the generated section that it documents
      defaults an operator configuration supersedes.
- [ ] 5.5 Add a test that a configured rebinding appears in the help overlay,
      and that it appears in a pane hint strip that advertises the rebound
      action — the picker and interaction strips carry such bindings; the
      compose surface has no strip today.
- [ ] 5.6 Confirm `scripts/lint-tui-binding-documentation.sh` still passes and
      still fails on drift, given the generated section now carries the
      defaults statement.

## 6. Presets

- [ ] 6.1 Declare the preset mechanism: a named set of rows carrying the
      capability class its rows apply to, contributing nothing under any other,
      and applied in the order the configuration names them.
- [ ] 6.1a Express shipped presets as configuration files embedded in the
      binary, and obtain their rows by parsing that embedded text through the
      same parser an operator's configuration goes through, rather than
      constructing the rows in code.
- [ ] 6.1b Add a test that parses every shipped preset, so a preset that does
      not parse fails the repository's checks rather than reaching a release.
- [ ] 6.1c Treat a run-time parse failure of a shipped preset as an internal
      invariant violation, and add a test that it is not reported as a fault in
      the operator's configuration file — the text is a compile-time constant,
      so failure implicates our artifact rather than anything they wrote.
- [ ] 6.2 Ship the two presets, both for the disambiguating class — `Enter`
      inserts a newline with `Shift+Enter` sending, and `Enter` inserts a
      newline with the primary-modified `Enter` sending.
- [ ] 6.3 Add a test that a preset declaring the disambiguating class
      contributes nothing when the probe reports the other, leaving the compiled
      defaults in force.
- [ ] 6.4 Add a test that each shipped preset leaves sending reachable in every
      context it touches, so a preset cannot ship in the state its class
      declaration exists to prevent.
- [ ] 6.5 Add a test that the compiled defaults are identical across both
      capability classes when no preset is applied and no row is configured, so
      the claim that this change alters nothing out of the box is asserted
      rather than assumed.
- [ ] 6.6 Verify a preset end to end under a real capable terminal with the
      pty-debug procedure, including that the help overlay shows the moved send
      chord.

## 7. Validation and pre-flight

- [ ] 7.1 Reject a configuration under which no chord reaches the quit action
      under either capability class, with an error naming the file in effect and
      the class, so the answer is the same at startup and at pre-flight.
- [ ] 7.1a Build the effective table for each capability class where no probe
      outcome is available, so pre-flight has both to inspect.
- [ ] 7.1b Report, from `agentmux check configuration`, any action a context's
      compiled rows declare that a class's effective table leaves unreachable
      there, naming the action, the context, and the class — as a finding rather
      than a rejection, and once rather than twice where it holds under both.
- [ ] 7.1c Add tests for a finding under one class only, a finding under both,
      and quit unreachable under one class alone being rejected. Add a test that
      displacing an action by rebinding its only chord is reported and still
      loads, and that declaring the chord against no action is reported the same
      way, since the report describes the outcome rather than judging intent.
- [ ] 7.2 Extend `agentmux check configuration` to validate the binding group
      through the same read-only loader and effective-file lookup.
- [ ] 7.3 Add a test that pre-flight reports the physical `ui.toml` the lookup
      selected when more than one layer supplies a copy.
- [ ] 7.4 Add a test that loading writes no configuration artifact, so the
      compiled defaults are never scaffolded to disk.

## 8. Documentation and reconciliation

- [ ] 8.1 Document the binding configuration in `documentation/usage/tui.md`:
      the file and group, the chord grammar, the capability columns, the
      presets, the symbolic modifier, unbinding, and a worked example.
- [ ] 8.2 Update `src/tui/actions/README.md`: a configuration is the present
      successor rather than a future one, and the statement that rows carry no
      capability field now holds because the defaults do not vary rather than
      because variance is unexpressible.
- [ ] 8.3 Update `src/configuration/README.md` for the binding group, including
      why it merges over compiled defaults while files still replace whole.
- [ ] 8.4 Reconcile `tui-binding-configuration` against what shipped, checking
      each requirement rather than the ones this list happened to name.
- [ ] 8.5 Reconcile `tui-action-bindings` against what shipped, including that
      every surviving statement about capability neutrality is scoped to the
      defaults wherever it appears.
- [ ] 8.6 Reconcile `ui-surface-configuration` against what shipped.
- [ ] 8.7 Sweep this change's own artifacts for claims the implementation
      falsified, within each file as well as across them.
- [ ] 8.8 Record the macOS delivery question in the terminal capability matrix,
      and flip the default resolution of the symbolic modifier only if evidence
      supports it.
