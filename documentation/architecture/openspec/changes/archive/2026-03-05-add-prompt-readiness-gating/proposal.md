# Change: Add prompt-readiness template gating for relay delivery

## Why

Quiescence alone does not guarantee a target session is at an input-ready
prompt. Some agent UIs render stable output while waiting on user confirmation
or while still in a non-input state.

## What Changes

- Add optional per-member prompt-readiness templates in bundle configuration.
- Gate relay injection on both quiescence and prompt-readiness when a template
  is configured for a target session.
- Add prompt-readiness timeout reporting distinct from pure quiescence timeout.
- Use one multiline `prompt_regex` against inspected pane tail text.

## Impact

- Affected specs: `session-relay`
- Affected code:
- `src/configuration.rs`
- `src/relay.rs`
- `tests/unit/configuration.rs`
- `tests/integration/session_relay_delivery.rs`
