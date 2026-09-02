## 1. Action vocabulary and context resolution

- [x] 1.1 Add `src/tui/actions/` with an import-only `mod.rs` hub, plus
      `action.rs`, `bindings.rs`, and `context.rs`.
- [x] 1.2 Define the `Action` enum covering every behavior currently invoked
      from the six handlers in `src/tui/input.rs`. Derive the member list from
      the existing arms so no behavior is dropped in translation.
- [x] 1.3 Define `BindingContext` and `binding_context(&AppState)`, encoding
      overlay-over-mode precedence and focused-field selection within a mode.
- [x] 1.4 Test that `binding_context` returns the overlay context when an
      overlay is open over each screen mode, and the field-scoped context
      otherwise.
- [x] 1.5 Test that applying an `Action` to state produces the behavior with no
      `KeyEvent` constructed, establishing the resolution/behavior split.
- [x] 1.6 Export `Action` from `agentmux::tui` and add action application to the
      public `Workbench` facade alongside `dispatch_event`. Export
      `BindingContext` with it: this task first said to keep the context
      internal, and operator direction reversed that, since keeping it internal
      would have forced the table's own invariants into inline tests in the
      module they check. The table itself stays internal.
- [x] 1.7 Test the public boundary from `tests/unit/tui.rs` — a caller naming
      only public types applies an action through `Workbench` and observes the
      behavior, with no `KeyEvent` constructed.
- [x] 1.8 Test that applying an action directly and dispatching the chord bound
      to it produce the same resulting state, so the two paths cannot diverge.
      Sweep the surfaces rather than sampling one, and compare the whole public
      read surface: a projection narrow enough to omit a field is a way the
      paths could drift without the test noticing.

## 2. Binding table

- [x] 2.1 Define the table as (context, chord) rows carrying the action and the
      display section. Carry no capability field: nothing varies by probe
      outcome, so a per-row flag would be unused machinery. The global context
      is one of the keys, holding the rows that survive any open surface.
- [x] 2.2 Populate every row from the current handlers, preserving each
      context's existing bare-`Enter` action unchanged.
- [x] 2.3 Declare `Enter`, `Shift+Enter`, and `Ctrl+Enter` explicitly for every
      context that binds `Enter`, with both modified chords bound to the same
      action that context binds to `Enter`. The events and help overlays bind
      no `Enter` action and declare none of the three, as the design's binding
      table already records; neutrality holds there because all three forms are
      equally inert, not because they share an action.
- [x] 2.4 Test that every context declaring an `Enter` row also declares both
      modified rows, so no context can inherit modified-`Enter` behavior by
      omission.
- [x] 2.5 Test capability neutrality directly: for every context, the action
      resolved for `Shift+Enter` and `Ctrl+Enter` equals the action resolved for
      `Enter`. Assert it over the whole table rather than per context, so a row
      added later cannot quietly reintroduce divergence.
- [x] 2.6 Test that `Ctrl+J` resolves to insert-newline in exactly the contexts
      that own a text draft, and in no other. That is three contexts, not the
      two this task first named: the interaction choice pane forwards typed
      characters into the write draft today, and its unguarded `Ctrl+J` arm
      inserts there too rather than being inert, so dropping the row would
      change behavior.
- [x] 2.7 Test that `Shift+Enter` in the compose `Message` field sends the
      message. This is the regression detection introduced and this change
      repairs, so assert it rather than assuming it follows from 2.5.
- [x] 2.8 Declare `Ctrl+C` and `F1` as global rows — the two chords `handle_key`
      tests ahead of every overlay today — and test that each resolves to its
      action with the picker, the events overlay, and the help overlay open.
      Without this the chords either lose their reach or survive as an
      unmodelled early return, and both defeat the single source of truth.

## 3. Dispatch rewiring

- [x] 3.1 Replace the six handlers' key-condition match arms with lookup
      against the table followed by action application, walking
      `binding_lookup_order` so the global rows are consulted before the
      contextual ones. Keep event-shape handling (paste, mouse, non-`Press`
      filtering) in `input.rs`.
- [x] 3.2 Verify the full existing TUI test suite passes unchanged except for
      the deliberate modified-`Enter` cases, and record which tests changed and
      why.
- [x] 3.3 Teeth-check the omission guarantee: remove one context's
      `Shift+Enter` row and confirm task 2.4's test fails.
- [x] 3.4 Confirm `handle_key` retains no chord-specific early return ahead of
      the table, so the global rows are the only thing granting a chord reach
      across surfaces. Task 1.8's sweep is what holds this: a chord answered
      ahead of lookup makes dispatch diverge from the action the table names,
      so the confirmation is mechanical rather than a reading of the source.

## 4. Generated help and hints

- [x] 4.1 Define the help presentation rule as a function distinct from
      `binding_context`: it selects every reachable context, not the dispatched
      one. Generate the help overlay from it, grouped by declared display
      section in declaration order. The rule takes no `AppState`, which is what
      makes task 4.2's property structural rather than merely tested. Its
      context order is presentation's own, not `BindingContext::ALL`'s, which
      pairs each surface with its dispatch precedence rather than with what
      reads well.
- [x] 4.2 Test that the generated help contains the compose, interaction, and
      picker bindings, and that its binding set and order are identical
      whichever context the overlay was opened from. This is the regression
      that a context-filtered help would produce, so assert it directly rather
      than inferring it from a passing render. Assert it through the workbench,
      which a context-filtered implementation would have to read: comparing the
      state-free catalogue against itself proves nothing.
- [x] 4.3 Generate `picker_hint_line` and the interaction write-pane hint from
      the table, retiring their hand-rolled context-sensitive labels. These stay
      filtered to the context they annotate. Generated wording is longer than
      the shorthand it replaces, so both strips wrap rather than truncate: the
      picker's is laid out before the vertical split so its row count sizes its
      own section, and it packs at entry boundaries so a binding is never split
      across rows. Every packed row is reserved, with no cap: a cap clips
      whichever binding lands last and discloses nothing. Where a single entry
      cannot fit a row even alone, it degrades to the unqualified description
      rather than being clipped, which is asserted at the widths that force it.
- [x] 4.4 Test that a hint strip presents only its own context's bindings,
      pinning the asymmetry with help rather than leaving it to reviewer memory.
      Assert as well that the chord a strip prints resolves to the behavior it
      names, which is what a row shadowed within its own context would break.
- [x] 4.5 Test that generated presentation does not read the keyboard-enhancement
      probe outcome, so no capability-conditioned behavior enters through the
      rendering path. Two halves: the generation functions take no state, and
      the rendered overlay's binding columns are byte-identical under all three
      outcomes. The capability report is asserted to differ, or the second half
      would pass on a page that ignored the outcome entirely.
- [x] 4.6 Compare the rendered help overlay before and after generation and
      resolve any readability regression before proceeding. Generation is
      taller and wider than the transcript it replaces: one line per behavior
      where the transcript combined directions ("Arrows/Home/End: Move
      cursor"), and every chord spelled out. In two columns the result
      overflowed at terminal sizes the old overlay fitted, pushing the
      keyboard-capability report off the bottom. Resolved by folding the
      redundant modified `Enter` forms out of the printing, shortening the
      behavior wording to the transcript's register, and moving to three
      gutter-separated columns with the reference material in the third.
      Asserted, not just inspected: one inline test renders the overlay at the
      target geometry and checks that the retained hand-written material
      survives and that the columns stay separated.

## 5. Documentation single-sourcing

- [x] 5.1 Add a generated, delimited binding section to
      `documentation/usage/tui.md` and populate it from the table. The generator
      is an example reaching the table through the public exports alone, which
      compiles the claim that a caller outside the crate can build its own
      binding reference; it emits the delimiters itself, so the marker text has
      one definition and the lint cannot disagree with it about where the block
      starts. The guide's hand-written binding lists are retired in the same
      move: a generated block beside a transcribed one is two copies in one
      file, which is worse than the one it replaced.
- [x] 5.2 Add a repository lint that regenerates that section and fails on
      mismatch, following the existing repo lint conventions. It runs after the
      Rust lints rather than with the cheap repository checks, because the
      declaration it reads is Rust and the only honest way to read it is to run
      it. Guard against the vacuous pass: a generator that emits no bindings
      would agree with an emptied block about nothing, so producing none is a
      failure however well it matches. `--fix` lives in the lint rather than the
      generator, so locating the block has one implementation in both
      directions.
- [x] 5.3 Teeth-check the lint: change a binding row without regenerating and
      confirm the lint fails. Teeth-check its guards too, since a guard that
      cannot fire is the failure it was written against: an emptied table, a
      missing marker, and markers in the wrong order each fail, and the emptied
      table fails `--fix` as well rather than writing an empty block.
- [x] 5.4 Remove the now-duplicated binding prose from `src/tui/README.md` and
      the interaction pane hint text, leaving architecture description only.
      Every runtime operator prompt is generated, not only the interaction
      pane's: the footer's mode-switch hint and the startup status line named
      chords too, and a prompt that names a chord is the consumer the spec
      requires to read the table. They are generated rather than deleted, since
      the guide is not on screen when the operator needs them.
      - The choice pane's block title had never been reached by the hint work
        at all. A block title does not wrap, so the pane advertises its two
        decisions and drops them whole rather than cut when the width runs out,
        leaving navigation to the help overlay.
      - The footer's mode-switch hint reads the dispatch context rather than a
        fixed one. A pane hint annotates the surface it sits on; the footer
        spans the workbench and says what pressing a key would do right now.
      - The startup line is composed in the render layer rather than seeded
        into `AppState`, so the state layer keeps its independence from the
        action layer. `Action::apply` calls `AppState` methods, and reversing
        that direction for one string would be a poor trade.
      - The keyboard-enhancement paragraphs in `src/tui/README.md` and the
        usage guide keep their chords. They are the durable statement of the
        neutrality contract, which is about those chords rather than a
        reference to them.
      - The runtime capability report is not covered by that exception. It once
        ended by naming the chord that inserts a newline under every outcome,
        which is a chord paired with a behavior and would go false the moment
        the row moved. `keyboard.rs` now answers only for delivery — how a key
        reaches the TUI — and the help renderer generates that line. It prints
        only while every drafting context binds the same chord, since the claim
        it makes is universal.

## 6. Spec and behavior reconciliation

- [x] 6.1 Update `src/tui/README.md` to describe the action layer, the binding
      table as single source of truth, and the context precedence rule. Carried
      at the strength of the normative delta rather than the task's summary,
      per the standing rule that the requirement governs where task phrasing is
      narrower: that dispatch tests no chord ahead of the table, that the table
      declares defaults rather than compile-time facts, and that the context is
      a value derived from state, which is what makes precedence testable
      rather than a property of control flow.
- [x] 6.2 Record two durable constraints in `src/tui/README.md`, where they
      survive proposal archival: that action application goes through `AppState`
      methods rather than fields, and that default bindings are
      capability-neutral because capability variance belongs to a binding
      configuration rather than to compiled defaults. Both are recorded with
      their reasons, since a constraint whose rationale is archived with the
      proposal reads as an arbitrary rule and gets relaxed. The neutrality
      entry keeps the non-obvious half: leaving a modified form reserved and
      unbound would itself be capability-conditioned.
- [x] 6.3 Update the usage guide's terminal-capability section: the probe
      outcome reports what the TUI determined, not a terminal limitation, and no
      longer implies any behavior difference. Remove the per-mode
      modified-`Enter` split it currently documents, and check the surrounding
      prose for claims a failed probe cannot support. The section now opens on
      the delivery fact rather than on a binding, and closes on where terminal
      differences are meant to live, so an operator reading it learns that
      neutrality is a choice with a successor rather than a limitation. The
      newline rationale is retained without naming the chord, and links to the
      generated section that does.
- [ ] 6.4 Confirm the `tui-surface` delta matches shipped behavior, and that the
      five unrelated keyboard-enhancement scenarios remain byte-identical to the
      live spec. **Blocked on evidence, not deferred.** Confirming shipped
      behavior requires observing the `KeyboardEnhancement::Active` branch and
      real modified-`Enter` delivery, and neither is reachable from the test
      suite or from `pty-debug`, which implements no Kitty keyboard protocol.
      The operator's terminal pass is the evidence; asserting conformance
      before it would be asserting the one thing nothing has measured.
