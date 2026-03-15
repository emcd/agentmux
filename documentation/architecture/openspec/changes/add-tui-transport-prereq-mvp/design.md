## Context

Current TUI behavior is send/look/list focused. The next milestone (history
viewport) needs deterministic TUI-side contracts for sender identity and status
mapping. Relay stream protocol mechanics are intentionally split into an
adjacent change.

## Goals

- Lock one sender-resolution contract for `agentmux tui` startup.
- Lock CLI surface contract for `agentmux tui --sender` as precedence anchor.
- Lock bare `agentmux` dispatch behavior (TTY => TUI, non-TTY => help + non-zero).
- Lock TUI state mapping for delivery outcomes and reconnect behavior.
- Keep MVP scope same-bundle and fail-fast.

## Non-Goals

- Cross-bundle delivery/history implementation.
- Defining relay stream wire protocol in this change.
- Defining relay polling API/cursor contracts in this change.

## Decisions

- Decision: TUI sender identity resolution precedence is:
  1. CLI `--sender`
  2. local testing override sender file
     `.auxiliary/configuration/agentmux/overrides/tui.toml`
     (debug/testing mode only)
  3. normal config sender file `<config-root>/tui.toml`
  4. runtime association fallback
  5. explicit validation error when unresolved.

- Decision: TUI delivery state vocabulary is fixed:
  - `accepted`: async enqueue accepted (from send ack)
  - `success`: terminal delivered outcome
  - `timeout`: terminal timeout outcome
  - `failed`: terminal failure outcome
  - relay terminal `dropped_on_shutdown` maps to
    `failed` with `reason_code=dropped_on_shutdown`.

- Decision: reconnect/errors are explicit. No silent degrade behavior:
  - transport unavailability is surfaced as stable machine-readable errors,
  - same-bundle scope violations remain validation errors,
  - reconnect starts fresh stream handling for MVP (no implicit local replay),
  - TUI does not silently switch bundle scope.

- Decision: relay stream protocol and `hello` registration model are specified
  in adjacent change `add-relay-stream-hello-transport-mvp`.

- Decision: bare `agentmux` invocation dispatches by terminal context:
  - interactive TTY with no subcommand starts TUI workflow,
  - non-TTY with no subcommand prints help and exits non-zero.

## Risks / Trade-offs

- Trade-off: splitting transport protocol into adjacent change increases
  proposal count, but avoids mixing cross-cutting relay protocol decisions into
  TUI-specific UX/state contracts.
- Risk: sender precedence can confuse operators if undocumented.
  Mitigation: lock one precedence order in spec and CLI/TUI help text.

## Migration Plan

1. Land this TUI-focused prerequisite change.
2. Land adjacent relay stream/hello transport change.
3. Implement `todos/tui/4` history viewport against both locked contracts.
