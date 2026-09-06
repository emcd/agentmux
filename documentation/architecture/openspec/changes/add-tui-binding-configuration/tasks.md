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

- [x] 3.1 Add the raw TOML shapes for the `[bindings]` group to
      `src/configuration/raw.rs`, keeping them private to the module.
- [x] 3.2 Extend `UiConfiguration` with the validated binding group, the preset
      selection, and the macOS primary-modifier selection.
- [x] 3.3 Parse and validate the group in the `ui.toml` loader, leaving the
      file's existing resolution, absence, and malformed-file behavior
      untouched.
- [x] 3.4 Add tests for an absent group, an absent `ui.toml`, a group naming an
      unknown action, an unknown context, an unknown preset, and an unparseable
      chord. The shipped-preset registry is intentionally empty until task 6.2
      populates it, so every named preset is unknown and is refused; accepting
      names provisionally would be a check with nothing behind it.
- [x] 3.5 Add a test that an invalid binding group applies no binding from that
      configuration, rather than the rows preceding the invalid one.
- [x] 3.6 Reject an unrecognized key at any level of the group, and a value
      outside the permitted set for `primary-modifier-on-macos`, rather than
      ignoring either.
- [x] 3.7 Add a fixture test carrying the configuration shape the specification
      documents, character for character, so the published shape cannot drift
      from what the loader accepts. While no binding set shipped, the whole text
      was held under test by asserting that the unshipped preset was the sole
      reason it was refused, with a second fixture derived from the first by
      dropping that line asserting the parsed configuration row by row. Task 6.2
      shipped the set the example names and collapsed the two into one
      successful load of the whole text. Asserting what the shape produces in an
      effective table belongs with task 4.1, where the table exists.

      The fixture is a copy of the specification's example, so it catches the
      loader drifting away from the published shape but not the specification
      being edited without it. Extracting the example from the specification at
      test time would close that, at the cost of binding the test to a path that
      moves when the change is archived.
- [x] 3.8 Derive each context's permitted action set from its compiled rows, and
      reject a configured or preset row binding an action the context does not
      declare, with an error naming the action and the context.
- [x] 3.9 Add a test that binding a contextually inert action is rejected, using
      an action whose effect is guarded on another focused field, and a test
      that a new chord for an action the context already declares is accepted.

## 4. Effective binding table

- [x] 4.1 Build the effective table from the compiled default rows, the applied
      presets, the configured rows, and the probe outcome, resolving the
      symbolic modifier for the running platform as it builds. The capability
      class and the platform arrive as arguments rather than being probed, so
      the table is buildable for either without a terminal.
- [x] 4.2 Order configured rows ahead of preset rows and preset rows ahead of
      compiled rows within a context, and leave `binding_lookup_order`
      unchanged. The preset tier is a build parameter rather than a hole, so no
      named set exists to fill it yet and every caller passes an empty slice,
      but the tier resolves like any other and populating it changes nothing
      about how a lookup answers.
- [x] 4.3 Implement explicit unbinding, so a chord configured against no action
      is inert rather than falling through to its preset or compiled default.
- [x] 4.4 Add tests for row-level merge: an unnamed default survives, a
      configured row wins over the compiled row it shadows, a configured row
      wins over a preset row, and a configured contextual row does not shadow a
      compiled global row.
- [x] 4.5 Add tests for the capability columns: a single value applies to both
      classes, a class-qualified value applies to its class, and an omitted
      class keeps its compiled default.
- [x] 4.6 Teeth-check 4.2 by ordering configured rows after compiled rows and
      confirming the test that covers a configured chord shadowed by a broader
      compiled row fails.

## 5. Wiring the consumers

- [x] 5.1 Hold the effective table on the workbench, built where the loaded
      configuration and the probe outcome are both in hand. The configuration
      arrives as a launch option and the table is rebuilt when the probe
      outcome is recorded, so a run cannot start having silently ignored a
      binding group the operator wrote.
- [x] 5.2 Resolve dispatch against the effective table. Precedence between
      contexts stays in the dispatch layer, since the table answers for one
      context at a time.
- [x] 5.3 Generate the help overlay and the pane hint strips from the effective
      table. A context presents its configured rows ahead of whatever of its
      compiled rows the configuration left standing; a compiled row drops out
      where a higher tier claimed the keystroke that row is written as.
- [x] 5.4 Keep `examples/tui-binding-reference.rs` and the usage guide reading
      the default table, and state in the generated section that it documents
      defaults an operator configuration supersedes. The example reads
      `default_help_bindings`, which takes no effective table rather than
      defaulting one, so a runtime-specific table is not passable there.
- [x] 5.5 Add a test that a configured rebinding appears in the help overlay,
      and that it appears in a pane hint strip that advertises the rebound
      action — the picker and interaction strips carry such bindings; the
      compose surface has no strip today. A second test covers the half that
      leaves open: the compiled row a configured chord took over stops being
      advertised.

      The displacement test is sited on the message field rather than the
      picker. The picker's two columns declare the same rows, so configuring
      one column leaves the other's chord standing and the catalogue rightly
      keeps presenting it — which is behavior, not a defect, and would have
      made the assertion a false alarm.
- [x] 5.6 Confirm `scripts/lint-tui-binding-documentation.sh` still passes and
      still fails on drift, given the generated section now carries the
      defaults statement. Checked in both directions: an edit to the committed
      block and a changed compiled row each fail it.

## 6. Presets

- [x] 6.1 Declare the preset mechanism: a named set of rows carrying the
      capability class its rows apply to, contributing nothing under any other,
      and applied in the order the configuration names them. The class is
      carried by the rows, in the format an operator writes: a set for the
      disambiguating class states the `enhanced` column and no other, so
      `ConfiguredBinding::for_class` drops every row under the other. That is
      what makes the restriction structural rather than a separate class field
      a set could contradict its own rows with. Rows are concatenated in the
      order the configuration names the sets, which the tier's existing
      last-row-wins rule turns into later-supersedes-earlier.
- [x] 6.1a Express shipped presets as configuration files embedded in the
      binary, and obtain their rows by parsing that embedded text through the
      same parser an operator's configuration goes through, rather than
      constructing the rows in code. Text lives in `data/bindings/`, embedded
      with `include_str!`, read by `embedded_binding_preset` through
      `validate_binding_group`. A set may name neither presets nor the macOS
      primary modifier: the first would recurse, and the second is the
      operator's selection rather than a set's to make.
- [x] 6.1b Add a test that parses every shipped preset, so a preset that does
      not parse fails the repository's checks rather than reaching a release.
      `config::ui::every_shipped_binding_set_parses`, which also refuses a set
      that parses to no rows.
- [x] 6.1c Treat a run-time parse failure of a shipped preset as an internal
      invariant violation, and add a test that it is not reported as a fault in
      the operator's configuration file — the text is a compile-time constant,
      so failure implicates our artifact rather than anything they wrote.
      `ConfigurationError::MalformedEmbeddedArtifact` carries no path, so no
      consumer downstream can name their file from it; the four sites that map
      configuration faults outward each classify it as internal rather than as
      validation. The TUI startup mapping needed an explicit arm: its catch-all
      would have called it `validation_invalid_arguments`.
- [x] 6.2 Ship the two presets, both for the disambiguating class — `Enter`
      inserts a newline with `Shift+Enter` sending, and `Enter` inserts a
      newline with the primary-modified `Enter` sending. Named
      `enter-newline-shift-enter-sends` and
      `enter-newline-primary-enter-sends`; the second name is the one the
      published configuration shape already used. Their arrival collapses task
      3.7's two fixtures into one successful load of the documented shape.
- [x] 6.3 Add a test that a preset declaring the disambiguating class
      contributes nothing when the probe reports the other, leaving the compiled
      defaults in force. Swept over the whole keystroke space rather than over
      the chords the set names, since a set leaking into the other class could
      displace a chord it does not name. Carries a clause asserting the set does
      change something under the class it declares, without which a set that
      parsed to a no-op would satisfy the sweep by doing nothing anywhere.
- [x] 6.4 Add a test that each shipped preset leaves sending reachable in every
      context it touches, so a preset cannot ship in the state its class
      declaration exists to prevent. Reachability is asked only over the
      keystrokes a terminal in that class can deliver: under the standard class
      a modified `Enter` arrives as a bare `Enter`, so counting a `Shift+Enter`
      row as an answer there would have missed exactly the state in question.
      What "sending" means is read from the context's compiled bare-`Enter` row
      rather than named in the test. A sibling test asserts each set does move
      sending off that chord, without which the check could not fail.
- [x] 6.5 Add a test that the compiled defaults are identical across both
      capability classes when no preset is applied and no row is configured, so
      capability neutrality is asserted rather than assumed. Asserted between the
      two classes directly, rather than against a table each was separately
      compared to.

      What it proves is that the classes agree with each other, and nothing
      more. It is not evidence that the defaults are the ones that shipped
      before this change, and reading it that way would let a class comparison
      stand in for evidence about what an operator loses. Group 8 withdraws
      modifier variants, and this test stays green through that — correctly,
      since neutrality is preserved and historical identity was never what it
      measured.
- [ ] 6.6 Verify a preset end to end under a real capable terminal with the
      pty-debug procedure, including that the help overlay shows the moved send
      chord. **Half done, and the remaining half is not reachable from here.**
      `pty-debug` reports `Kitty keyboard protocol: unsupported` on every run,
      so it resolves the standard class and both shipped sets are inert in it by
      construction. Verified there: the set loads through the real binary,
      pre-flight accepts its name and refuses an unknown one, and under the
      standard class `Enter` still sends at dispatch rather than inserting a
      newline. The presentation half was verified with an operator configuration
      carrying the set's two rows for both classes, which is the only way to
      make those rows apply in a standard-class terminal: the overlay then reads
      `Enter / Ctrl+J: Message: insert newline` and
      `Shift+Enter / Ctrl+Enter: Message: send`, with the moved chord leading
      its line, and the portable-newline note correctly disappears. What remains
      is one run under a terminal that reports the protocol active, with a
      shipped set named, which needs the operator.

## 7. Validation and pre-flight

- [x] 7.1 Reject a configuration under which no chord reaches the quit action
      under either capability class, with an error naming the file in effect and
      the class, so the answer is the same at startup and at pre-flight.
      Refused as the group is validated, so every path that loads `ui.toml`
      inherits it rather than pre-flight and startup each carrying a copy. The
      refusal reads out of the same sweep that produces every other finding,
      which is what stops the two disagreeing about whether quit is reachable.

      **The refusal is wired up, correct, and cannot be triggered by any
      configuration this grammar can express.** Quit sits on a compiled control
      chord, which matches every modifier set containing `Ctrl`; two of the six
      modifier flags a terminal can report — `Hyper` and `Meta` — have no
      spelling in the chord grammar, so `Ctrl+Hyper+C` is unclaimable and keeps
      quitting however much else an operator claims. Reachability has to answer
      for what dispatch does, and dispatch delivers that keystroke to the quit
      row, so refusing there would refuse a configuration that works.

      Asserted rather than left to be rediscovered, in
      `no_configuration_the_grammar_can_express_takes_quit_away` and its loader
      and pre-flight counterparts. The guard is kept because the requirement
      asks for it and because a quit row written as one exact keystroke would
      trip it; whether a requirement that cannot fire should stand as written is
      a question Group 8 answers by making the rows exact, rather than one to
      settle by weakening the runtime. Once they are, this note describes a
      condition that no longer holds and task 8.7 reworks it.
- [x] 7.1a Build the effective table for each capability class where no probe
      outcome is available, so pre-flight has both to inspect.
      `EffectiveBindings::for_each_class`. Group 6 gave this teeth: the shipped
      sets are enhanced-only, so the two tables genuinely differ now.
- [x] 7.1b Report, from `agentmux check configuration`, any action a context's
      compiled rows declare that a class's effective table leaves unreachable
      there, naming the action, the context, and the class — as a finding rather
      than a rejection, and once rather than twice where it holds under both.
      Printed as `binding finding:`, deliberately not added to the `findings`
      vector that answers for the exit status, so a report cannot fail a run.
      The action and context are named in the operator's own vocabulary, since
      the file is what they act on.

      Candidate keystrokes are derived from the compiled rows themselves and
      expanded over the whole modifier domain a terminal can report, not over
      the domain the grammar can spell. The consequence is worth stating: a
      behavior sitting on a chord shape that matches more than it spells is
      permanently reachable, so findings can only ever name a behavior whose
      every chord is written as one exact keystroke. In the compiled table that
      is the `Enter` family alone, which reaches sending in the compose message
      field and completion acceptance in the `To` field. Everything else is
      unreportable, not because nothing is checked but because nothing an
      operator writes can take it away.
- [x] 7.1c Add tests for a finding under one class only, a finding under both,
      and quit unreachable under one class alone being rejected. Add a test that
      displacing an action by rebinding its only chord is reported and still
      loads, and that declaring the chord against no action is reported the same
      way, since the report describes the outcome rather than judging intent.
      The last pair is asserted as an equality between the two findings rather
      than as two separate expectations, so they cannot drift apart.
- [x] 7.2 Extend `agentmux check configuration` to validate the binding group
      through the same read-only loader and effective-file lookup.
- [x] 7.3 Add a test that pre-flight reports the physical `ui.toml` the lookup
      selected when more than one layer supplies a copy. Asserted in both
      directions: the shadowed copy configures nothing, so reading it instead
      would produce no finding as well as the wrong path.
- [x] 7.4 Add a test that loading writes no configuration artifact, so the
      compiled defaults are never scaffolded to disk. Compares the whole
      configuration tree with contents before and after, with a guard that the
      fixture actually put a tree there — two empty listings would otherwise
      compare equal.

## 8. Exact chord matching

Group 7's reachability question is what surfaced the problem, and exact matching
shrinks the answer: the keystroke expansion over modifier combinations collapses
to the keystroke a row is written as.

- [x] 8.1 Enumerate what the compiled table over-matches today — every row whose
      chord shape accepts a keystroke the row is not written as — and record the
      list, so the decision below is made against what is there rather than
      against what is remembered.

      Recorded at `agentmux:artifacts/51`. 114 rows over-match, withdrawing
      6,734 keystrokes: 100 rows matching a key under any modifier at 63 each,
      and 14 matching a character under any superset of `Ctrl` at 31 each. The
      finding that shaped the rest of the group: the table had almost no
      exactly-written rows, the only ones being the six `Enter` rows in
      `compose-to` and `compose-message`. Exactness therefore changes the
      matching shape of all but six rows rather than trimming edges.
- [x] 8.2 Confirm against that list that no modifier variant is declared back.
      The operator decided this on 2026-09-06, before the enumeration rather
      than after it: so long as the chords the table is written as keep working,
      which variants stop working is not something they want to adjudicate row
      by row. So the enumeration is evidence rather than a decision point — its
      job is to show that nothing a row IS written as was lost, and to supply
      the list of withdrawn keystrokes for release notes.

      A variant is declared back only if 8.1 turns up one whose loss breaks a
      chord the table declares, which would mean exactness had been applied
      wrongly rather than that the variant was wanted.

      None was. Every over-matching shape keeps its written form — an
      any-modifier row keeps the bare key, a control row keeps `Ctrl+<key>`, a
      character row keeps bare and `Shift` — so nothing a row is written as was
      lost. No withdrawn keystroke is the written form of another row either;
      had one been, dispatch would already have been returning that other row's
      action.

      All 6,734 withdrawn keystrokes are observable to an operator who happens
      to press one, and the enumeration is the list of them — `Alt+Enter` in the
      interaction and picker contexts, `Ctrl+Shift+R`, modified `F2` through
      `F5`, and the modifier variants of every navigation and escape key among
      them. Release notes are written from the enumeration rather than from this
      task. `Ctrl+Shift+C` and `Ctrl+Shift+J` are singled out only as task 8.8's
      targeted real-terminal cases, being the ones a habitual operator is most
      likely to have in their fingers; they are not the extent of the change.
- [x] 8.3 Make every non-typing chord shape match exactly the keystrokes its
      written form denotes — one for a key with a modifier set, two for a bare
      character. The shapes that exist only to reproduce a handler condition go
      away rather than gaining a narrower condition.
- [x] 8.4 Keep a bare single character denoting that character both bare and
      carrying `Shift`, covering the fixed-action character rows as well as the
      typing rows, and add a test that a shifted character still reaches its row
      in both. This is the one place exactness would break something a terminal
      actually does.
- [x] 8.4a Resolve an operator's bare single-character chord to the same two
      keystrokes the compiled row denotes, rather than to the bare form alone.
      Without this the configured row claims one of the two and the compiled row
      keeps answering for the other — the very condition Group 8 exists to
      remove, reappearing in the one shape exempted from it. Add a test that
      configuring a character intercepts its shifted arrival and that the
      compiled row it displaced reaches nothing.
- [x] 8.4b Teeth-check 8.4a by resolving the configured chord to the bare form
      only, and confirm the test fails. The two sides denoting the same set is
      the whole of the guarantee here, and a symmetry that is never exercised
      asymmetrically is not known to hold.
- [x] 8.5 Add a test that a row's action is unreachable through that row under
      any modifier set outside what its written form denotes, swept over the
      modifier domain rather than over a chosen sample. Written against the
      denoted set rather than against the modifiers a row names, since for a bare
      character those differ: `Shift` is denoted without being named, and a test
      phrased the other way would demand the opposite of task 8.4.
- [x] 8.6 Add a test that the help overlay and dispatch agree about a rebound
      chord: where presentation drops a compiled row, no keystroke reaches that
      row's action through it. This is the contradiction that motivated the
      change, so it is asserted rather than assumed to have gone.
- [x] 8.7 Simplify the Group 7 reachability keystroke expansion to the keystroke
      each row denotes, and rework the fixtures that were written against broad
      matching. The quit refusal and the displacement findings become reachable
      conditions again; assert them where they were previously asserted to be
      inexpressible.
- [ ] 8.8 Verify under a real terminal that a rebound control chord no longer
      leaves its old behavior on a modified variant, since this is the one
      change in the arc an operator can notice without configuring anything.

## 9. Documentation and reconciliation

- [ ] 9.1 Document the binding configuration in `documentation/usage/tui.md`:
      the file and group, the chord grammar, the capability columns, the
      presets, the symbolic modifier, unbinding, and a worked example.
- [ ] 9.2 Update `src/tui/actions/README.md`: a configuration is the present
      successor rather than a future one, and the statement that rows carry no
      capability field now holds because the defaults do not vary rather than
      because variance is unexpressible.
- [ ] 9.3 Update `src/configuration/README.md` for the binding group, including
      why it merges over compiled defaults while files still replace whole.
- [ ] 9.4 Reconcile `tui-binding-configuration` against what shipped, checking
      each requirement rather than the ones this list happened to name.
- [ ] 9.5 Reconcile `tui-action-bindings` against what shipped, including that
      every surviving statement about capability neutrality is scoped to the
      defaults wherever it appears.
- [ ] 9.6 Reconcile `ui-surface-configuration` against what shipped.
- [ ] 9.7 Sweep this change's own artifacts for claims the implementation
      falsified, within each file as well as across them.
- [ ] 9.8 Record the macOS delivery question in the terminal capability matrix,
      and flip the default resolution of the symbolic modifier only if evidence
      supports it.
