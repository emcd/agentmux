# TUI Guide

`agentmux tui` is an interactive terminal surface for recipient discovery,
messaging, session inspection, and pane snapshots.

## Launch

```bash
agentmux tui
```

Optional session selector:

```bash
agentmux tui --as-session user@GLOBAL
```

Startup behavior:

- `agentmux tui` attempts relay auto-start when the resolved relay socket is
  unavailable.
- Auto-start uses the same resolved runtime roots as the active TUI launch
  (`--configuration-directory`, `--state-directory`, `--inscriptions-directory`).

Relay lifecycle:

- If the TUI auto-spawns a relay, it stops that relay when the TUI exits (any
  exit path: quit key or terminal signal). Shutdown is graceful and active — it
  prunes the tmux sessions the relay owns and reaps the tmux server — so the
  coder sessions that relay is hosting are torn down with it.
- A TUI-auto-spawned relay is therefore an ad hoc, single-operator convenience,
  not a durable or shared relay. For a relay that must outlive any single TUI
  (or be shared across clients), run it under a service manager (e.g. systemd)
  or via `agentmux host relay` directly. A relay that is already running when
  the TUI starts is left untouched on TUI exit.

## Screen Modes

The TUI has two co-equal top-level screen modes. Exactly one is active at a
time, and the footer shows which. Per-mode cursor, draft, and scroll state is
preserved across switches.

- **Communication** (default) — chat history and compose (`To` + `Message`)
  for send/receive workflows.
- **Interaction** — an interaction-target header, the look snapshot, a Write
  input, and choice decisioning for operator-driven session inspection.

When Interaction mode has no target, the header shows a placeholder hint:
open the picker and choose a session.

## Keybindings

### Terminal keyboard capability

Terminals differ in whether they can report a modified `Enter` as something
other than a bare `Enter`. Without the Kitty keyboard protocol, `Enter`,
`Shift+Enter`, and `Ctrl+Enter` arrive as the same bytes, and nothing
downstream can tell them apart. Inserting a newline therefore has a chord of
its own rather than a modified `Enter`: it is the one editing action that would
otherwise be unreachable on a terminal without the protocol.
[Default bindings](#default-bindings) names it.

At startup the TUI probes for that protocol once and enables key
disambiguation when the terminal advertises it. The help overlay reports the
outcome under "Keyboard Capability":

- `Kitty keyboard protocol: active` — the terminal advertised the protocol, so
  modified keys arrive distinctly.
- `Kitty keyboard protocol: unsupported` — the terminal answered the probe
  without advertising the protocol, so they arrive collapsed.
- `Kitty keyboard protocol: probe failed` — the probe could not complete: no
  controlling terminal, an I/O failure, or no reply before the query timeout.
  This says nothing about the terminal; it may well support the protocol, and
  the TUI simply could not determine it.

Which terminals and multiplexers land in which bucket has not been measured
yet; that survey is tracked as `todos/tui/62`.

#### What the outcome tells you

What the TUI determined about your terminal, and nothing more. It is not a
statement that your terminal is limited, and it does not predict a difference
in what the TUI will do.

The default bindings are deliberately arranged so that it cannot. Wherever
`Enter` does something, the modified forms do the same thing; where `Enter`
does nothing, none of them do. So whether your terminal reports the three
distinctly or as one is invisible in the defaults, on every surface.

That neutrality is a choice about where terminal differences belong, not a
verdict that the distinction is worthless. Disambiguation is exactly what makes
a modified chord bindable at all, and operator-configurable bindings are the
intended successor to the compiled defaults. When they arrive, a terminal that
reports the three distinctly will be able to do three different things with
them — because you asked it to, in a configuration you control, rather than
because the TUI guessed from a probe.

### Default bindings

<!-- BEGIN GENERATED BINDINGS -->
<!-- Generated from the binding table in src/tui/actions/bindings.rs.
     Regenerate with: scripts/lint-tui-binding-documentation.sh --fix
     Do not edit between these markers; the pre-commit lint rejects drift. -->

The modified `Enter` forms are folded into the bare one they always match; see
[Terminal keyboard capability](#terminal-keyboard-capability).

#### Modes

- `Ctrl+C` — Quit from anywhere
- `F1` / `Esc` — Toggle help
- `F2` — Open picker (sessions)
- `F3` / `Esc` — Toggle events overlay
- `F4` — Switch Communication / Interaction
- `F5` — Open picker (bundles)
- `Ctrl+R` — Refresh recipients

#### Communication Mode

- `Ctrl+A` / `Home` — To: field start
- `Ctrl+E` / `End` — To: field end
- `Ctrl+U` — To: clear field
- `Ctrl+Space` — To: trigger completion
- `Tab` — Focus next field
- `Shift+Tab` — Focus previous field
- `Enter` — To: accept completion
- `Up` — To: previous completion
- `Down` — To: next completion
- `Left` — To: cursor left
- `Right` — To: cursor right
- `Backspace` — Delete before cursor
- `PgUp` — Scroll chat history up
- `PgDn` — Scroll chat history down
- `Type` — Insert into focused field
- `Ctrl+A` / `Home` — Message: line start
- `Ctrl+E` / `End` — Message: line end
- `Ctrl+J` — Message: insert newline
- `Enter` — Message: send
- `Esc` — Message: snap history
- `Up` — Message: cursor up a line
- `Down` — Message: cursor down a line
- `Left` — Message: cursor left
- `Right` — Message: cursor right

#### Interaction Mode

- `Ctrl+J` — Write: insert newline
- `Enter` — Write: dispatch to active target
- `Left` — Write: cursor left
- `Right` — Write: cursor right
- `Up` — Write: cursor up / scroll
- `Down` — Write: cursor down / scroll
- `Home` — Write: line start
- `End` — Write: line end
- `Backspace` — Write: delete before cursor
- `PgUp` — Scroll look snapshot up
- `PgDn` — Scroll look snapshot down
- `Type` — Insert into write input
- `Enter` — Choice: resolve selected option
- `Left` — Choice: previous request
- `Right` — Choice: next request
- `Up` — Choice: previous ACP option
- `Down` — Choice: next ACP option
- `c` / `C` — Choice: resolve as cancelled

#### Picker

- `Esc` / `F2` / `F5` — Close picker
- `Enter` — Bundle col: switch bundle
- `Tab` / `Shift+Tab` / `Left` / `Right` — Switch column
- `Down` — Next entry in column
- `Up` — Previous entry in column
- `Backspace` — Delete from column filter
- `Type` — Filter focused column
- `Enter` — Session col: insert or open look

<!-- END GENERATED BINDINGS -->

### What the bindings do not say

The section above is the whole of the default chords. These are the surrounding
facts a binding row cannot carry:

- The mouse wheel scrolls chat history.
- `To` takes recipients by grammar: `session` routes within the active bundle,
  `session@bundle` to a named bundle, and `session@GLOBAL` to a relay-wide
  user. Comma-separate multiple recipients; accepting a completion commits the
  `, ` delimiter for you.
- Which Interaction pane a chord reaches depends on state. The Write input is
  live when it holds text, or when the target has no pending choice requests;
  the choice pane is live when the Write input is empty and a request is
  pending. With an empty Write input and nothing pending, the vertical cursor
  keys scroll the look snapshot rather than moving a cursor.
- Entering Interaction mode without an active session auto-opens the picker on
  the session column, so a target can be chosen immediately. Close the picker,
  or choose a session, to reach the Write input.
- A resolved choice carries `outcome=selected` with the chosen option, or
  `outcome=cancelled` with no option.
- Filtering a picker column narrows it to case-insensitive matches on name or
  display name and resets the selection to the first match. Switching columns
  clears the filter.

## The Picker

The picker is a single window with two side-by-side columns: **Bundles** (left)
and **Sessions** (right, the active bundle's recipients). It opens focused on
one column or the other depending on which chord opened it. The focused column
is marked with a `▶` and a highlighted title.

The picker header surfaces a one-line bundle status in CLI-style key=value
format (`bundle=NAME hosted=yes|no state=up|down ...`) and color-codes it:

- green: hosted and healthy,
- yellow: hosted and degraded,
- red (bold): hosted but `state=down` (sessions failed to start),
- gray: not hosted (never started or shut down).

This distinguishes `hosted=true, state=down` (bundle is up but every session
failed startup) from `hosted=false, state=down` (bundle has not been started).

Each session row reflects the session's per-session readiness from the relay
list payload. Sessions that are not yet ready render dimmed and gain a
trailing `[not ready]` marker so the state is legible even without color.

Committing a bundle selection keeps the picker open and hands focus to the
re-enumerated session column; committing the already-active bundle is a no-op.
Committing a session selection means different things per mode: in
Communication mode it inserts the selected recipient into `To`, and in
Interaction mode it opens the Interaction screen for the selected identity —
the relay `Look` runs synchronously, so the look pane is populated with recent
session history before the Write input takes focus.

The active bundle is highlighted and labeled `[active]`. Switching to a
different bundle:

- replaces the active bundle context (header `Bundle:` indicator reflects the
  new bundle),
- rebuilds the relay stream session with the new bundle,
- clears bundle-scoped state (recipients, last-selected recipient, bundle
  status, look snapshot, pending choices, chat history, delivery state,
  write draft),
- triggers a recipient refresh against the new bundle; if the new bundle is
  unhosted/unreachable, the refresh fails fast and surfaces a relay error in
  the status pane (the bundle context stays switched).

The picker remembers the most recently committed session by name across
close/reopen and across recipient list refreshes. When the prior target is no
longer present in the current list, the session selection falls back
deterministically to the first available session.

The picker lists one bundle's sessions at a time (the active bundle).
Cross-bundle targeting is governed by policy scope: reaching sessions in
another bundle requires the `all` scope for the operation, and the relay
denies insufficient scope with `authorization_forbidden`.

## Status and Outcome Vocabulary

Connection state labels:

- `relay_unavailable`: relay socket not reachable
- `relay_timeout`: relay reachable but request timed out

Delivery outcomes:

- `accepted`: locally accepted and pending terminal completion
- `success`: terminal success
- `timeout`: terminal timeout
- `failed`: terminal failure with reason/reason_code when available

## Usage Notes

- Successful send clears `To` and `Message`.
- Recipient completion supports both `@`-triggered suggestions and the manual
  completion trigger.
- Completion candidates span every bundle visible to the operator, not just the
  active one: the active bundle's recipients are offered alongside relay-wide
  `session@bundle` candidates from the other available bundles. These are
  refreshed on the same cycle as the recipient list; bundles the operator lacks
  scope to enumerate are simply omitted from the suggestions.
- Look snapshot rendering is transport-aware:
  - tmux look snapshots render line payloads directly.
  - ACP look snapshots render structured entries by kind:
    `user`, `agent`, `cognition`, `invocation`, `result`, `update`.
- Write dispatch from Interaction mode routes through relay `raww`; delivery is
  asynchronous. The write is acknowledged immediately as `queued`, and its
  terminal outcome arrives later as a `delivery_outcome` stream event.
- Terminal outcomes are sourced from relay completion updates keyed by
  `message_id`, and `raww` writes are tracked and deduped the same way as chat
  sends.
- Choice requests are rendered from relay `choices.snapshot`,
  `choices.requested`, and `choices.resolved` events.
- Replay is at-least-once; the TUI deduplicates pending choice rows by
  `choice_request_id`.
- Pending choice rows are ordered FIFO by `enqueued_at` (rows without one sort
  last), with ties broken deterministically by `choice_request_id`, so
  choice-request navigation visits a session's requests oldest-first regardless
  of the order events arrive or replay.
- Choice decisions are ACP-native and explicit: selected option ids are
  forwarded verbatim via `choices.pick`; cancelled decisions omit
  `option_id`.
