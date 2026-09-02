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
- [ ] 4.3 Generate `picker_hint_line` and the interaction write-pane hint from
      the table, retiring their hand-rolled context-sensitive labels. These stay
      filtered to the context they annotate.
- [ ] 4.4 Test that a hint strip presents only its own context's bindings,
      pinning the asymmetry with help rather than leaving it to reviewer memory.
- [ ] 4.5 Test that generated presentation does not read the keyboard-enhancement
      probe outcome, so no capability-conditioned behavior enters through the
      rendering path.
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

- [ ] 5.1 Add a generated, delimited binding section to
      `documentation/usage/tui.md` and populate it from the table.
- [ ] 5.2 Add a repository lint that regenerates that section and fails on
      mismatch, following the existing repo lint conventions.
- [ ] 5.3 Teeth-check the lint: change a binding row without regenerating and
      confirm the lint fails.
- [ ] 5.4 Remove the now-duplicated binding prose from `src/tui/README.md` and
      the interaction pane hint text, leaving architecture description only.

## 6. Spec and behavior reconciliation

- [ ] 6.1 Update `src/tui/README.md` to describe the action layer, the binding
      table as single source of truth, and the context precedence rule.
- [ ] 6.2 Record two durable constraints in `src/tui/README.md`, where they
      survive proposal archival: that action application goes through `AppState`
      methods rather than fields, and that default bindings are
      capability-neutral because capability variance belongs to a binding
      configuration rather than to compiled defaults.
- [ ] 6.3 Update the usage guide's terminal-capability section: the probe
      outcome reports what the TUI determined, not a terminal limitation, and no
      longer implies any behavior difference. Remove the per-mode
      modified-`Enter` split it currently documents, and check the surrounding
      prose for claims a failed probe cannot support.
- [ ] 6.4 Confirm the `tui-surface` delta matches shipped behavior, and that the
      five unrelated keyboard-enhancement scenarios remain byte-identical to the
      live spec.
