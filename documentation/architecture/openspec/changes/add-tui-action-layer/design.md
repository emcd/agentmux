## Context

`src/tui/input.rs` resolves keys through six handler functions chained by
statement order in `handle_key`: the global quit and help toggle, then picker,
then events overlay, then help overlay, then the active screen mode. Each
handler's match arms carry the key, the context condition, and the state
mutation together. There is no name for "what the operator asked for" between
the `KeyEvent` and the `AppState` method call.

Three consequences follow, and all three are already visible in the tree:

1. **The precedence chain is encoded in statement order.** Which surface owns a
   key when overlays are stacked is readable only by tracing early returns.
2. **Binding knowledge is duplicated with no link.** `Ctrl+J` is bound in two
   handlers and restated in the help overlay (twice), the interaction write
   hint, the usage guide, the TUI README, and the `tui-surface` spec.
3. **Uniformity is accidental.** `picker_hint_line` already hand-rolls a
   context-sensitive action label (`session→To` versus `session→look`) — the
   exact thing a table would express — while the three `KeyCode::Enter` arms
   disagree on whether modifiers are guarded, which nobody chose.

The keyboard capability detection that landed for `todos/tui/22` supplies the
missing input: the TUI now knows at runtime whether the terminal reports
modified keys distinctly. Until it did, a binding grammar could not honestly
name `Shift+Enter`.

## Goals / Non-Goals

**Goals:**

- One named `Action` vocabulary, public, with key-to-action resolution
  separable from action-to-behavior and reachable from outside the crate.
- One context-scoped binding table that dispatch, help rendering, and pane
  hints all read.
- Explicit, per-context declaration of modified `Enter`, with capability-neutral
  defaults.
- Explicit, testable binding-context precedence.
- A lint that fails when operator documentation and the table disagree.
- A table shaped as *defaults*, so operator-configurable bindings can be added
  additively rather than by contradicting this change.

**Non-Goals:**

- **Changing any operator-visible behavior.** Every chord invokes the action it
  invokes today, on every terminal.
- The binding configuration format itself. Configurable bindings are the
  intended successor to this change, not part of it; landing a config parser
  first would leave the duplication this change exists to remove.
- Chord sequences (multi-key prefixes). Nothing in the current surface needs
  them and the table can gain them later without reshaping.
- Extracting widgets for embedding. This change makes that possible by
  separating resolution from behavior; the extraction itself is `ideas/tui/1`.
- Decomposing `AppState`. It is the other blocker for widgets, tracked as
  `todos/tui/63`; this change must not make it harder.

## Decisions

### One flat `Action` enum for now, expected to be grouped by widget domain later

Actions are named in a single enum rather than per-context enums. Context enters
as a key of the table, not as a type: the picker's `Enter` already resolves to
different actions by screen mode, so the lookup has to vary over context, and
making context a type instead would force a host to speak six vocabularies.

That argument governs *binding resolution*. It does not govern the shape of the
action vocabulary, and the widget direction pressures that shape: a Communication
widget embedded in a host TUI has no mode toggle and no Interaction mode, so it
would receive a global vocabulary and ignore most of it. The likely revision is
grouping — `ComposeAction`, `ChatHistoryAction`, `RecipientPickerAction`, and an
app-level enum composing them — which matches the widgets a host would embed.

Deferred rather than done now because the grouping should follow the widget
boundaries rather than guess them, and partitioning a flat enum afterwards is
mechanical with no compatibility obligation pre-1.0. Implementation should keep
action application dispatching through `AppState` methods rather than reaching
its fields, so neither the grouping nor the state decomposition has to unpick it.

### Actions dispatch through state methods, not state fields

Action application calls `AppState` methods (`send_message`,
`insert_newline_if_message`, and so on) rather than reading or writing fields
directly. Today's handlers already do this almost everywhere.

This is what keeps the action layer cheap to live with while `AppState` is still
a single struct holding compose, history, picker, interaction, shell, and relay
state together. Method names survive a field regrouping; field accesses do not.
Without the constraint, the state decomposition would have to rewrite the action
layer it just gained.

### `BindingContext` is a value computed from state, not an if-chain

A single `binding_context(&AppState) -> BindingContext` replaces the implicit
precedence in `handle_key`. Overlay contexts outrank mode contexts; within a
mode, the focused field selects the context.

This converts an ordering invariant into a value that can be asserted
directly. The current chain is correct, but its correctness is a property of
where the early returns sit — a reviewer cannot check it without simulating
the function.

### Default bindings are capability-neutral, and capability variance is configuration's job

Capability-conditioned behavior is not hardcoded. The rule that decides the
space:

- A modified chord bound to the **same** action as its unmodified form is
  capability-independent — it behaves identically whether or not the terminal
  disambiguates, because collapsing to bare `Enter` reaches the same action.
- A modified chord bound to a **different** action is capability-dependent, and
  behaves differently on the two terminal classes.

Neutrality therefore requires that *every* modified-`Enter` chord map to its
context's `Enter` action. Leaving `Ctrl+Enter` "reserved and unbound" would not
have been neutral: an enhanced terminal would do nothing while an unenhanced one
collapsed it to `Enter` and acted. This yields:

| Context | `Enter` | `Shift+Enter` | `Ctrl+Enter` |
|---|---|---|---|
| Compose `To` | accept completion | same as `Enter` | same as `Enter` |
| Compose `Message` | send | same as `Enter` | same as `Enter` |
| Interaction write | dispatch write | same as `Enter` | same as `Enter` |
| Interaction choice | resolve option | same as `Enter` | same as `Enter` |
| Picker bundles | switch bundle | same as `Enter` | same as `Enter` |
| Picker sessions | insert / open look | same as `Enter` | same as `Enter` |
| Events, help overlays | unbound | unbound | unbound |

`Ctrl+J` keeps the insert-newline action in both multi-line fields, unchanged.

This produces **zero** operator-visible behavior change, and it repairs a
regression: activating disambiguation made `Shift+Enter` in `Message` stop
sending on capable terminals, because the compose arm rejects modified `Enter`.
That was shipped as an unintended consequence and described in the spec
descriptively rather than named as a regression.

*Alternative considered:* `Shift+Enter` → insert newline, matching the
prevailing chat-client idiom. Rejected as a hardcoded capability-conditioned
binding: it differs from `Enter` → send, so it is reachable only on terminals
that disambiguate, which is exactly the divergence that must be expressed as
configuration rather than compiled in. It is a good *default* to offer once
bindings are configurable, not a default to compile.

*Assumption, worth recording:* a terminal without the Kitty protocol reports no
modifier for `Shift+Enter`. Under capability-neutral defaults this assumption
carries no risk — if some terminal did report `SHIFT` without full protocol
support, the modified row matches and invokes the same action anyway.

### Capability variance belongs to a two-column binding configuration

The probe outcome is deliberately given no influence over the default table. A
binding that should differ by terminal class is a configuration question: a
binding configuration with two value columns, one for the disambiguating case
and one for the default case, with rows identical in both unless the operator
says otherwise.

That configuration is not part of this change. Because the table here defines
*defaults* rather than fixed bindings, adding it later is an additive
requirement rather than a contradiction of this one.

*Consequence:* no per-row capability flag is carried, and there is no
capability-aware presentation rule. An earlier draft had both; with neutral
defaults nothing varies, so the flag would have been unused machinery and the
presentation rule would have had nothing to annotate. The probe outcome is
still reported to the operator — that requirement lives in `tui-surface` — as a
statement about the terminal, not about any binding.

### The action vocabulary and the binding context are public; the table is not

`Action` is exported from the TUI's public API and the public `Workbench`
facade gains direct action application. Without that, the capability's stated
rationale — an embedding host driving the TUI by action — is not actually
secured by anything: `AppState` is crate-private and `Workbench` today exposes
only `dispatch_event`, so a requirement phrased as "a caller applies an action
to state" would be satisfied by a crate-private function that no host can
reach.

`BindingContext` is public alongside it, with a `default_binding` lookup that
answers what a chord means on a given surface. An earlier draft kept the
context internal, on the grounds that embedding needs the vocabulary rather
than the TUI's precedence model, and that exporting it would fix a shape before
`ideas/tui/1` has said what it needs. Operator direction reversed that: the
public API is meant to grow toward reuse, and one intended effect is that the
test suite lives under `tests/` rather than inside implementation modules.
Keeping the context internal would have forced the table's own invariants —
capability neutrality, the omission guarantee — into inline tests in the very
module they check.

The table itself stays internal. Its rows, chord patterns, and display sections
are the shape most likely to move, and nothing outside the crate needs them in
order to ask what a chord does.

### Presentation is a separate rule from dispatch precedence

Dispatch resolves exactly one context and routes the chord there. Help
presentation does **not** use that value.

The two must be separate because the help overlay outranks the mode beneath it
for dispatch — that is what makes `Esc` close help rather than snap the chat
history. If help presentation reused the dispatched context, opening `F1` would
make the active context `HelpOverlay` and the generated overlay would list only
its own handful of navigation keys, losing the compose, interaction, and picker
reference the two-column help exists to provide.

So help renders across every reachable context, grouped by declared display
section in declaration order, with capability-unreachable rows annotated. Pane
hint strips keep context filtering, because a hint strip annotates the surface
the operator is acting on right now.

*Alternative considered:* keeping one context notion and special-casing the
help overlay to render "the context underneath". Rejected — the underneath
context is one of several the reference should show, so the special case would
still under-present, and it would reintroduce ordering-dependent behavior of
exactly the kind this change removes.

*Alternative considered:* sorting rows for display. Rejected — the current help
overlay groups bindings pedagogically (modes, then compose, then grammar), and
sorting would discard an editorial judgment that is worth keeping.

### The documentation check is a lint, not a test

`documentation/usage/tui.md` gains a generated, delimited block; a repository
lint regenerates it and fails on mismatch. This follows the existing repo
convention for invariants (line counts, OpenSpec delta retention) rather than
encoding a docs-drift check as a unit test.

*Alternative considered:* set-equality between prose and table, tolerant of
wording. Rejected as unfalsifiable in practice — it passes on paraphrases that
have already drifted in meaning.

### Module layout

A new `src/tui/actions/` subtree with an import-only `mod.rs` hub, alongside
`action.rs` (the enum), `bindings.rs` (the table), and `context.rs`
(`BindingContext` and its resolution). `input.rs` retains the event-shape
handling (paste, mouse, non-`Press` filtering) and becomes a dispatcher over
resolved actions.

## Risks / Trade-offs

- **A half-migrated dispatch would have two sources of truth, which is the
  defect being fixed.** → Land the table and dispatch for every context in one
  slice; sequence only the *consumers* (dispatch, then help and hints, then the
  lint) across tasks.
- **A neutral default table can be mistaken for "capability detection was
  pointless".** → It is not: activation widens the set of chords a terminal can
  deliver, which is what makes modified chords bindable at all. The defaults
  decline to *use* that width; a binding configuration is what will.
- **Neutral defaults keep `Shift+Enter` sending a message, which some operators
  will expect to insert a newline.** → That is today's behavior on terminals
  without the protocol and was the behavior everywhere before detection landed,
  so nothing regresses. The chat-client idiom becomes available as soon as
  bindings are configurable, which is the successor change.
- **A generated help overlay can regress readability relative to hand-written
  prose.** → Keep declaration-order grouping and section headings in the table
  so the generated output can reproduce the current layout; treat a visible
  diff in the rendered help as a review checkpoint, not an afterthought.
- **The table can grow into a configuration format inside this change.** → The
  non-goals fix the boundary: the table is an in-code default set with no
  parser, no file format, and no per-operator overrides here.
- **The flat `Action` enum may need regrouping for widgets, and the state
  decomposition will touch the same files.** → Action application dispatches
  through `AppState` methods rather than fields, so both later changes edit the
  action layer's *shape* without rewriting its behavior.
- **`input.rs` is 343 lines against an 800-line warn threshold; extraction adds
  files without obviously shrinking the total.** → The win is single-sourcing,
  not line count. Expect the total across `actions/` plus `input.rs` to exceed
  today's `input.rs`, and judge the change on whether the duplicated prose
  disappears.

## Migration Plan

1. Land `actions/` (enum, table, context resolution) with dispatch rewired and
   every context's `Enter`/`Shift+Enter`/`Ctrl+Enter` row declared explicitly.
2. Rewire the help overlay and both pane hint strips to read the table.
3. Add the usage-guide lint and regenerate the documented block.

Each step is independently revertible, and none changes operator-visible
behavior. Step 1 restores `Shift+Enter` in `Message` to sending on capable
terminals, which detection had silently stopped.

No rollback plan is needed for behavior, since there is no behavior change to
roll back. Reverting is reverting a refactor.

## Open Questions

- Should the lint also cover `src/tui/README.md`, or only the operator-facing
  usage guide? The README describes architecture rather than bindings, so it
  may not need to be generated — but it does currently restate `Ctrl+J`.
- What else, beyond `Action` application and `BindingContext`, does the
  embedding host in `ideas/tui/1` need from the public facade? The widget-surface
  question decides whether anything more is required, and does not block this
  change.
