# Change: Per-coder Pty terminal protocol configuration

## Why

The Pty transport currently hardcodes the `TERM` env var to
`xterm-256color` and `COLORTERM` to `truecolor` for every child
coder it spawns (`src/pty/transport.rs:298-299`). Modern TUIs
(claude, codex, gemini, opencode) detect terminal capabilities
from `TERM` and adapt their keybindings, theme, and rendering
accordingly. Operators cannot opt in to richer terminal
protocols (CSI-u / kitty keyboard protocol, alacritty-specific
features, etc.) on a per-coder basis without a per-coder config
knob.

This work is moving from Tmux to Pty because we are aiming to
retire Tmux; per-coder terminal protocol negotiation is more
durable in `PtyTransport` than in Tmux.

## What Changes

- Add a `term-protocol` field to `[coders.<id>.pty]` accepting a
  closed enum of well-known terminal types.
- The Pty transport reads the field and sets `TERM` to the
  corresponding env-var value when spawning the child process.
- Default `term-protocol = "xterm-256color"` preserves today's
  behavior verbatim (operators who do not configure the field see
  no change in child env vars).
- `COLORTERM` continues to be hardcoded at `"truecolor"` (out of
  scope for this change; can be added later as a follow-up).

## Impact

- Affected specs: NEW `pty-terminal-protocols` capability
  (additive to existing `add-pty-transport`; the parent change
  remains at 13/61 tasks because §7+§8+§11 are bootstrap-side and
  unrelated to this enhancement).
- Affected code:
  - `src/configuration/types.rs` — add `TermProtocol` enum and
    `term_protocol` field to `PtyTargetConfiguration`.
  - `src/configuration/raw.rs` — add `term_protocol` field to
    `RawPtyTarget` and validated `PtyTarget`.
  - `src/configuration/targets.rs` — no validator changes needed;
    `deny_unknown_fields` on `RawPtyTarget` covers the schema.
  - `src/pty/transport.rs` — add `term_protocol` field to
    `pty::transport::PtyTargetConfiguration`; use it in the
    `cmd.env("TERM", ...)` call.
  - `src/relay/delivery/dispatch/worker.rs` — propagate the field
    when constructing the transport-side config.
  - `README.md` / operator docs — note the new field and the
    CSI-u / kitty-keyboard-protocol benefits for the project's
    TUIs.
- Out of scope:
  - Deep libghostty-vt effect-handler work for CSI-u query
    response (requires upstream library work; tracked in
    `todos/pty/5`'s upstream pin breakage ticket).
  - `COLORTERM` configurability.
  - Tmux transport parity (we are aiming to retire Tmux).