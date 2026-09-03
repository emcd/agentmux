# TUI Module

This module implements the interactive `agentmux tui` runtime.
It is developer-facing and describes code organization and state contracts.
User-facing usage details and keybindings are documented under
`documentation/usage/`.

## Surface Model

The TUI presents two co-equal top-level screen modes. Exactly one is active at
a time, and the active mode is shown in the footer.

- **Communication** — owns send/receive: chat history, compose (`To` +
  `Message`), pending-delivery indicator, send dispatch. Default startup mode.
- **Interaction** — owns operator-driven session inspection: an interaction
  target header, the look snapshot, a Write input (relay `raww`), and
  choice decisioning.

Help, the unified picker, and delivery events are overlays available in both
modes. The unified picker is a single window with a bundle column and a session
column, and has two entry points, one focused on each. Entering Interaction mode
without an active session auto-opens it on the session column.

## Action Layer

Every operator-invocable behavior is a member of one named vocabulary,
`Action`, declared separately from the chords that invoke it. The layer has two
independent halves:

- **Resolution** turns a chord plus the current state into an `Action`.
- **Behavior** applies an `Action` to `AppState`, and needs neither a
  `KeyEvent` nor a binding context.

The split is what lets a host drive the workbench without synthesizing terminal
events: a host that owns its event loop and its own bindings skips resolution
entirely and applies actions directly through `Workbench`. `Action` and the
application seam are public for that reason, not incidentally.

### The table is the single source of truth

`actions/bindings.rs` holds one row per (binding context, chord), carrying the
action and the display section that presents it. That table is the only place a
chord-to-action association is declared, and every consumer reads it: dispatch,
the help overlay, the pane hint strips, and — through a generated block — the
operator usage guide. Changing a row changes all of them with no other edit.

Dispatch tests no chord ahead of the table. A chord answered before lookup
would be a second declaration of what that chord means, which is the
duplication this layer exists to remove, so even the chords that reach their
behavior from every surface are ordinary rows under the global context.

The table declares **defaults**. Nothing here requires a chord's action to be
fixed at compile time; operator-configurable bindings are the intended
successor, and a configuration is expected to answer the same question
differently.

### Context precedence

The active binding context is resolved from `AppState` as a single value rather
than as an ordering of handler early-returns. An open overlay outranks the
screen mode beneath it; within a mode, the focused field selects the context.
Resolution consults the global rows first, then that context, so a global chord
is not shadowed by whatever is open over it.

Because the context is a value derived from state alone, it can be asked for
without dispatching an event — which is what makes the precedence rule
testable rather than a property of control flow.

### The help overlay presents more than a terminal usually shows

Generated presentation is one line per behavior, so the help overlay is taller
than the hand-written transcript it replaced: it needs 48 rows to render whole,
and most terminals do not open that tall. It is therefore drawn through a
viewport the operator moves, so everything it presents stays reachable at a
120x24 terminal without resizing one.

The chords that move that viewport are table rows like any other, declared
under the help-overlay context. Nothing about scrolling is authored in the
renderer — not the chords, and not the marker's advice about which chord to
press. That is the same rule the rest of this layer follows, applied to the one
behavior whose effect is confined to what is on screen.

One item is held to a stricter standard than reachability. The
keyboard-enhancement probe outcome is required to be *visible* in the overlay,
so it leads its column rather than closing it; a viewport shows a column from
its beginning.

### Two constraints that outlive this module's current shape

**Action application goes through `AppState` methods, never its fields.** The
state struct is expected to be regrouped, and a layer that reached into fields
would have to be rewritten each time it moved. Applying through methods also
keeps a behavior's guards in one place: several of them do nothing unless the
right field is focused, and that decision belongs to the state, not to the
action that asks for it.

**Default bindings are capability-neutral, and that is a statement about where
capability variance belongs rather than about what terminals can do.** In every
context that binds `Enter`, the `Shift+Enter` and `Ctrl+Enter` rows are
declared explicitly and reach the same action; a context that binds no `Enter`
action declares none of the three, leaving all three equally inert. No default
binding varies with the keyboard-enhancement probe outcome, so the defaults
produce identical observable behavior on a terminal that disambiguates modified
keys and one that does not.

The reason is not that the distinction is worthless — detection is what makes
modified chords bindable at all. It is that a compiled default is the wrong
place to spend it. Terminal classes are meant to diverge through a binding
configuration the operator controls, and a default that behaved differently on
two terminals would put that divergence somewhere they cannot reach. Note that
neutrality has to cover *every* modified form: leaving one "reserved and
unbound" would itself be capability-conditioned, since a disambiguating
terminal would do nothing while a non-disambiguating one collapsed the chord
onto `Enter` and acted.

See `actions/README.md` for the per-file shape and the reasoning that is local
to it.

## Module Map

- `mod.rs`
  - top-level run loop and terminal lifecycle.
- `state/mod.rs`
  - app state model/types, the `ScreenMode` enum, and shared helpers for
    runtime error/status mapping.
- `state/history.rs`
  - chat history/event tracking, pending-delivery accounting, stream-event
    dedupe, and paging/snap behavior.
- `state/compose/`
  - compose/raww draft editing, mode-switch transitions, picker retargeting,
    and dispatch helpers. Split by concern:
    - `mod.rs` — pure hub: submodule decls and re-exports of `AppState` and
      sibling-state items
    - `pickers.rs` — unified picker open/close (separate session-focused
      and bundle-focused entry points), column focus toggle,
      column-scoped filter, visible
      (filtered) index resolution, session-insert and bundle-commit
      selection, overlay toggles (`toggle_events_overlay`,
      `toggle_help_overlay`)
    - `editing.rs` — focus cycling, mode toggle, text/character editing,
      message cursor, `To` cursor + completion, `clear_compose_fields`
    - `interaction.rs` — interaction-mode entry, target set, raww draft
      editing + cursor, snapshot scrolling, interaction-region visibility,
      `overlay_snapshot_from_payload`, `render_transport_label`
    - `choices.rs` — look-choice selection/resolve,
      `ensure_pending_choice_selection`, `selected_look_choice*`,
      `look_pending_choices`, `submit_choice_decision`
    - `text_util.rs` — private `&str` cursor/line utilities shared across
      the compose submodules
- `state/relay.rs`
  - relay request/response plumbing, recipient refresh, and stream polling
    lifecycle.
- `actions/`
  - the named-action vocabulary and the binding context that decides
    which surface owns a chord. See `actions/README.md` for the
    resolution / behavior split. Split by concern:
    - `mod.rs` — pure hub: submodule decls and the `Action` re-export
    - `action.rs` — the public `Action` enum and `Action::apply`
    - `bindings.rs` — the default chord-to-action table, grouped by
      context, and `default_binding`, which reads it
    - `context.rs` — `BindingContext`, `binding_context`, and
      `binding_lookup_order`
    - `help.rs` — `help_bindings`, the whole table as the help
      overlay presents it, and `help_contexts`, the presentation
      rule that answers for every context rather than the active one
- `input.rs`
  - terminal event handling: which events carry a binding, and how a
    paste or a scroll reaches state. Keys resolve against `actions/`
    rather than being matched here, so no chord is named in this module.
- `keyboard.rs`
  - progressive keyboard-enhancement (Kitty keyboard protocol) capability
    detection: the `KeyboardEnhancement` outcome, its operator-facing
    description, and `KeyboardEnhancementSession`, which pushes the
    disambiguation flag when supported and pops it on drop. The
    description covers delivery only — how a key reaches the TUI under
    each outcome. What a key *does* is the binding table's, so this
    module names no chord-behavior pair; the help renderer generates the
    one the report used to carry.
- `render/`
  - per-mode pane rendering, overlays, and key help text. Split by area:
    - `mod.rs` — pure hub: submodule decls and `pub(crate) use frame::render`
    - `frame.rs` — top-level `render` entry, `render_header`/`render_main`/
      `render_footer`, frame-level layout constants
    - `communication.rs` — `render_communication_mode`, compose + chat
      history panes, peer resolution
    - `interaction.rs` — `render_interaction_mode`, target header, raww
      pane, look snapshot, structured entries, look choice lines
    - `overlays/`
      - `mod.rs` — pure hub: submodule decls
      - `picker.rs` — unified bundle+session picker overlay: bundle status
        header, column-scoped filter line, side-by-side bundle/session
        columns with focus indication, per-row readiness styling, hint
        strip
      - `events.rs` — events overlay (pending choices + delivery
        events)
      - `help.rs` — help overlay; bindings generated from the
        binding table, reference material hand-written
    - `cursor.rs` — active cursor, compose/raww cursor placement, position
      + column helpers, raww pane area
    - `geometry.rs` — shared measure/layout helpers (`centered_rect`,
      `split_workbench_rows`, `compute_compose_height`, titled blocks,
      `MessageLayout`, `compose_message_layout`,
      `compose_message_visible_start`, `wrap_text`, `raww_titled_block`)
- `target.rs`
  - recipient parsing/autocomplete and look-target resolution helpers.
- `workbench.rs`
  - the public `Workbench` facade over the internal `AppState`: launch-option
    plumbing plus the event-driven integration boundary for tests. It exposes
    `dispatch_event`, `apply_action` (the chord-free seam a host with its own
    bindings uses), `binding_context` / `binding_lookup_order` (which surface
    owns a chord now, and the order it is resolved in),
    focus/field/mode accessors, the relay-event ingestion
    seams (`record_stream_events`, `record_chat_events`), and read-only
    projections (e.g. `WorkbenchPendingChoice`, `pending_choices`). Callers
    drive it with contract-faithful inputs; the projections are a test/inspection
    boundary, not general domain APIs.

## Behavior

- recipient discovery from relay `list` responses,
- two co-equal screen modes (Communication, Interaction), with per-mode cursor,
  draft, and scroll state preserved across switches,
- explicit `To` recipient field with deterministic target parsing. The relay
  requires fully-qualified targets, so the client fills in the namespace before
  dispatch and always emits `session@bundle`:
  - bare `session` is qualified with the sender's bound bundle and emitted as
    `session@bound-bundle`,
  - canonical `session@bundle` is emitted verbatim, routing to the named bundle
    (peer bundle or the relay-wide `@GLOBAL` user registry),
  - a relay-wide sender (`@GLOBAL` principal, no bound bundle) has no bundle to
    qualify a bare target with, so a bare target is rejected at compose time with
    `validation_unqualified_target`; it must name each target's `@namespace`
    explicitly, because the relay derives `Send` routing for a relay-wide
    principal solely from target suffixes,
  - `parse_tui_target_identifier` rejects empty halves, `/`, and more than one
    `@` separator at compose time,
- async send workflow with local pending tracking and terminal outcome updates,
- session identity precedence:
  - `--as-session`
  - `default-session` from active `users.toml` configuration
  - no association fallback,
- bundle precedence (interactive launch does not require a bundle; a fresh
  install ships none, and the operator picks one in the picker):
  - `--bundle`
  - `default-bundle` from active `ui.toml` configuration
  - the first available bundle, else an empty browsing context,
- delivery outcome vocabulary:
  - `accepted`, `success`, `failed`, `not_submitted`, `submission_unknown`,
- recipient completion via `@` token triggers plus explicit manual trigger,
- `@`-prefixed tokens trigger immediate completion proposals after one suffix character,
- completion candidates span every bundle visible to the operator: the active
  bundle's recipients plus relay-wide cross-bundle `session@bundle` ids, fanned
  out eagerly on the same cadence as the recipient refresh; bundles the operator
  cannot enumerate (`authorization_forbidden`) degrade out silently,
- overlays:
  - help,
  - unified picker: a single window with two side-by-side columns — bundles
    (left) and the active bundle's sessions (right) — with a separate entry
    point focused on each; entering Interaction mode with no active session
    auto-opens it on the session column. A column-scoped filter narrows the
    focused column and resets its selection to the first match; switching
    columns clears the filter. The foot hint strip is generated from the
    binding table, filtered to the two picker contexts, and carries one
    description covering both modes rather than the mode-sensitive label it
    replaced.
  - delivery + choice events,
  - bundle column behavior: browses `available_bundles` (sourced from
    `load_bundle_group_memberships` at TUI launch) and highlights the active
    bundle. Committing a different bundle replaces the active bundle context —
    rebuilding the bundle-bound `RelayStreamSession`, resetting bundle-scoped
    state (recipients, `last_selected_recipient`, bundle status, look snapshot,
    pending choices, chat history, delivery bookkeeping, write draft), and
    triggering `refresh_recipients` on the new bundle — then keeps the picker
    open and hands focus to the re-enumerated session column so a session can be
    chosen in the same window. Committing the already-active bundle is a no-op
    that just hands focus to the session column. The picker enumerates one
    bundle at a time (the active one); relay-wide cross-bundle enumeration is
    tracked separately (todos/tui/47). Cross-bundle targeting for `Send` and
    `Look` is still handled via the
    `session@bundle` grammar in the `To` / look-target field: the relay resolves
    the peer bundle by suffix and authorizes the requester's capability at the
    uniform cross-bundle `all` scope (the same threshold for `send` and
    `look`; unknown peers/targets surface as `validation_unknown_bundle` /
    `validation_unknown_target`). `Raww` routes the same way: the relay derives
    the peer bundle from the look-target's `session@bundle` suffix and authorizes
    `raww` at the same uniform `all` cross-bundle scope (issues/relay/24).
    Because `Send` and `Raww` routing is suffix-based, the shared relay client
    omits the wire-envelope `namespace` on those frames for every caller (TUI and
    MCP alike); the browsing bundle survives only as the `List` / recipient-picker
    enumeration context and the `Look` namespace selector, never as a sender
    binding (so a relay-wide `@GLOBAL` sender shows no `Bundle:` field in the
    header),
- session-column commit is mode-aware rather than offering separate look and
  write keys:
  - Communication mode: insert the selected recipient into `To`,
  - Interaction mode: open the Interaction screen for the selected identity,
    running a synchronous relay `Look` so the look pane is populated with
    recent session history before the Write input takes focus,
- Interaction look-pane freshness: re-entering an already-targeted Interaction
  pane (`toggle_mode` -> `refresh_look_snapshot`) re-captures the look snapshot
  so the pane reflects current session state instead of the buffer frozen from a
  prior visit; re-capture preserves snapshot scroll and surfaces a relay failure
  to the operator,
- picker last-selected persistence: the most recently committed session target
  (Communication insert or Interaction open) is tracked by session name in
  `last_selected_recipient`; when the picker reopens or the recipient list
  refreshes, session selection is resolved by name against the current list and
  falls back deterministically to index 0 when the prior target is absent,
- picker bundle status header: one-line CLI-style key=value summary of
  `bundle.hosted`, `bundle.state`, `bundle.startup_health`, and reason code
  from `relay::ListedBundle`, color-coded into four severity buckets
  (`Healthy`/`Degraded`/`HostedDown`/`Unhosted`) so `hosted=true, state=down`
  (sessions failed to start) is visually distinct from `hosted=false,
  state=down` (not hosted),
- picker per-session startup failures: beneath the status header, one red
  `startup_failure session=… code=… reason=…` line per record from
  `bundle.recent_startup_failures` (capped by
  `STARTUP_FAILURE_PICKER_MAX_LINES`; the header's `startup_failure_count`
  carries the true total), so the operator reads why individual sessions
  failed rather than relying on the generic relay error path,
- picker per-session readiness: rows sourced from `relay::ListedSession`
  surface `ready` directly — ready rows render in default style, not-ready
  rows render dimmed (`DarkGray`/`DIM`) and gain a trailing `[not ready]`
  marker so the state survives color stripping,
- Interaction mode Write input dispatches raw writes to the active interaction
  target via relay `raww`,
- Write/choice region replacement: when the interaction target has pending
  choice requests and the Write input is empty, the choice
  decisioning pane occupies the region; otherwise the Write input occupies it,
- look snapshot rendering:
  - tmux targets: line snapshot rendering (`snapshot_lines`),
  - ACP targets: structured entry rendering by canonical kinds
    (`user`, `agent`, `cognition`, `invocation`, `result`, `update`),
- chat history viewport for sent/received messages,
- send workflow via relay `send`,
- look workflow via relay `look`,
- raw write workflow via relay `raww`,
- stable rendering for validation/runtime error codes,
- stream reconnect handling with explicit `relay_unavailable` (not reachable)
  and `relay_timeout` (reachable but unresponsive/saturated) status,
- choice lifecycle handling:
  - pending queue visibility from relay stream events,
  - replay-safe dedupe keyed by `choice_request_id`,
  - FIFO ordering by `enqueued_at` (tie-broken by `choice_request_id`),
    single-sourced through `compare_pending_choice_order` so the snapshot and
    upsert ingestion paths cannot diverge,
  - session-scoped Interaction-mode resolution using ACP-native
    `choices.pick` (`selected` with explicit `option_id` or `cancelled`),
- startup relay auto-spawn fallback when relay socket is unavailable, using the
  same resolved configuration/state/inscriptions roots as the active TUI
  launch,
- keyboard-enhancement capability detection: `run` probes the terminal once,
  after terminal setup and before the event loop reads any key, and pushes
  `DISAMBIGUATE_ESCAPE_CODES` when the terminal advertises the Kitty keyboard
  protocol. The probe writes a query and consumes its reply from the same input
  queue the loop drains, so it cannot run concurrently with the loop. The
  outcome (`Active` / `Unsupported` / `ProbeFailed`) is reported in the help
  overlay, because it decides whether a modified `Enter` is delivered
  distinctly from a bare one. Only the disambiguation flag is pushed; the
  remaining flags change which events are delivered at all and nothing in
  `input.rs` consumes them. Detection answers a delivery question and nothing
  else: it assigns no binding and reaches no behavior, so no probe outcome can
  resolve a chord differently. The outcome reports what the TUI determined
  about the terminal, not a difference in what the TUI does — see the
  capability-neutrality constraint under Action Layer for why the defaults are
  arranged so that it cannot.

## Stream and State Notes

- TUI identifies itself to the relay by `principal_id`
  (a `<session>@<namespace>` string); the relay does not gate
  stream clients on a class field.
- Stream event dedupe is keyed by stable identifiers in app state to avoid
  duplicate status/event lines after reconnect.
- `accepted` is process-local; terminal outcomes come from relay completion
  results/events.

## User Docs

- Usage guide: `documentation/usage/tui.md`
