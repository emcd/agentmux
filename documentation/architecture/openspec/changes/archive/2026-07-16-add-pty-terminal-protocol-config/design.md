# Design: Per-coder Pty terminal protocol configuration

## Context

The Pty transport currently hardcodes the `TERM` env var to
`xterm-256color` for every child coder it spawns
(`src/pty/transport.rs:298`). Modern TUIs (claude, codex,
gemini, opencode) detect terminal capabilities from `TERM` and
adapt their keybindings, theme, and rendering accordingly.
The hardcoded value means operators cannot opt in to richer
terminal protocols (CSI-u / kitty keyboard protocol,
alacritty-specific features, etc.) on a per-coder basis.

Reference material from operator (Claude Code terminal-config
docs, opencode config docs):

- Claude Code works in any terminal without configuration; it
  detects terminal features from `TERM` and from terminal-emitted
  escape sequences. Setting `TERM=xterm-kitty` opts the TUI into
  kitty-keyboard-protocol keybindings (Shift+Enter for newline,
  Option-as-Meta, etc.).
- OpenCode likewise adapts to terminal capabilities from `TERM`;
  it has no `TERM`-specific config knob documented.

The TUIs in the project's `data/configuration/coders.toml`
(claude, codex, gemini, opencode) all benefit incrementally from
`xterm-kitty` (better keybindings) but work fine without it.

This work is being moved from the Tmux transport to the Pty
transport because we are aiming to eventually retire Tmux;
per-coder terminal protocol negotiation is more durable in
`PtyTransport` than in `Tmux`.

## Goals / Non-Goals

- Goals:
  - Operators can configure the `TERM` env var on a per-coder
    basis for Pty-spawned children.
  - The default preserves today's behavior verbatim
    (`TERM=xterm-256color`).
  - The schema is self-validating: `deny_unknown_fields`
    catches typos in field names (e.g., `term-protocole`
    instead of `term-protocol`), and serde's enum-variant
    deserializer rejects unknown `term-protocol` values (e.g.,
    `"xterm-kittie"`).
- Non-Goals:
  - Auto-detect by parsing `initial_command` (too fragile;
    explicit per-coder config matches the established
    `cols`/`rows`/`wedge_detection`/`prime_timeout_ms` pattern).
  - Deep libghostty-vt effect-handler work for CSI-u query
    response (requires upstream library work; tracked in
    `todos/pty/5`'s upstream pin breakage ticket).
  - `COLORTERM` configurability (stays hardcoded `"truecolor"`
    for now; can be added later as a follow-up).
  - Tmux transport parity (we are aiming to retire Tmux).

## Decisions

### Decision: Closed enum, not free-form string

The `term-protocol` field is a closed enum of well-known terminal
types. Each variant maps 1:1 to the literal TERM env-var string
it emits.

- Rationale: the set of useful values is small (~6 entries); a
  closed enum is self-documenting, prevents typos, and matches
  the kebab-case / `deny_unknown_fields` config style used
  elsewhere in the project (e.g., `SessionType`,
  `TargetConfiguration`, `AcpChannel`).
- Alternatives considered:
  - **Free-form string** (`term = "xterm-kitty"`): simpler, more
    extensible. Rejected because operator typos become
    silently-wrong env-var values, and the project style uses
    self-validating enums for similar fields.
  - **String newtype** with a regex validator: middle ground.
    Rejected as over-engineered for ~6 known values.

### Decision: Default `xterm-256color`

The default `term-protocol` value is `xterm-256color`. This
preserves today's behavior verbatim — operators who do not
configure the field see no change in child env vars.

### Decision: Explicit config, no auto-detect

The field is always explicit per-coder; there is no auto-detect
by parsing `initial_command`. The set of TUIs that benefit from
CSI-u is not trivially derivable from the command name (e.g.,
`claude` could be a real Claude Code TUI or an unrelated
script). Explicit config keeps the seam auditable and matches
the established per-coder config pattern.

### Decision: New capability spec, not `runtime-bootstrap` retrofit

The proposal introduces a new capability spec
`pty-terminal-protocols` rather than retrofitting
`runtime-bootstrap`. The `runtime-bootstrap` spec governs XDG
roots, runtime layout, sender association, and relay config;
per-coder Pty transport config is a Pty-specific transport
concern, not a bootstrap concern. A new capability keeps the
spec surface focused and gives the Pty-side enhancements a
stable home.

### Decision: `COLORTERM` not in scope

The `COLORTERM` env var continues to be hardcoded to
`"truecolor"` (today's behavior). All four project TUIs work
fine with `COLORTERM=truecolor`; configurability is unnecessary
for the immediate operator need. If a future TUI requires a
specific `COLORTERM` value, this can be added as a follow-up
change that extends `pty-terminal-protocols`.

## Risks / Trade-offs

- **Risk**: an operator sets `term-protocol = "xterm-kitty"` but
  the child TUI doesn't fully use CSI-u because libghostty-vt
  doesn't emit CSI-u escape sequences in response to capability
  queries.
  - **Mitigation**: the proposal notes in the spec that deeper
    CSI-u handler work is out of scope and tracked in
    `todos/pty/5`. Operators see the same `TERM` value as they
    would in any other terminal emulator; the TUIs that benefit
    are those that interpret `TERM` for capability hints even
    without full CSI-u negotiation (per the Claude Code
    terminal-config docs, this is the established detection
    pattern).
- **Risk**: adding the field across raw/validated/transport/
  worker config sites is repetitive (~5 files).
  - **Mitigation**: the field is a single primitive; the
    cross-cutting change is small (<50 LOC total).

## Migration Plan

No migration. The default `xterm-256color` preserves today's
behavior verbatim. Operators opt in by setting `term-protocol`
on the affected coder.

## Open Questions

- Should `term-protocol` also affect the `on_xtversion` callback
  response (currently hardcoded to "agentmux-pty <version>")?
  E.g., `xterm-kitty` could return `"kitty"` instead. Out of
  scope for this proposal; can be revisited if a TUI uses
  XTVERSION for feature detection.
- Should there be a single `term-protocol` field or a richer
  sub-config (`[coders.<id>.pty.terminal]` with `term`,
  `colorterm`, `terminfo_overrides`)? Single field for now; can
  grow into a sub-config later if needed.