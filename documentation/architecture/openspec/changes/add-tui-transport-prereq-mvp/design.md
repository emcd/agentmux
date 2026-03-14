## Context

Current TUI behavior is send/look/list focused. The next milestone (history
viewport) needs deterministic transport and identity contracts so inbound
message display and delivery status updates are consistent across reconnects
and error paths.

## Goals

- Lock one sender-resolution contract for `agentmux tui` startup.
- Lock one structured relay->TUI event shape for history and delivery updates.
- Preserve existing relay delivery semantics while mapping them into TUI-ready
  statuses.
- Keep MVP scope same-bundle and fail-fast.

## Non-Goals

- Cross-bundle delivery/history implementation.
- Backward-compat shim layers for alternate TUI event formats.
- Durable relay event store in MVP.

## Decisions

- Decision: TUI sender identity resolution precedence is:
  1. CLI `--sender`
  2. `tui.toml` sender default
  3. runtime association fallback
  4. explicit validation error when unresolved.

- Decision: relay exposes a structured event retrieval flow for TUI with a
  canonical event union for:
  - `incoming_message`
  - `delivery_outcome`

- Decision: TUI maps transport states into one status vocabulary:
  - `accepted`: async enqueue accepted (from send ack)
  - `success`: terminal delivered outcome
  - `timeout`: terminal timeout outcome
  - `failed`: terminal failure outcome

- Decision: reconnect/errors are explicit. No silent degrade behavior:
  - transport unavailability is surfaced as stable machine-readable errors,
  - same-bundle scope violations remain validation errors,
  - TUI does not silently switch bundle scope.

## Risks / Trade-offs

- Trade-off: adding relay event contract increases protocol surface before
  history implementation, but prevents ad-hoc payload drift.
- Risk: event stream backlog handling could become ambiguous.
  Mitigation: define deterministic `since_event_id` + `limit` semantics.
- Risk: sender precedence can confuse operators if undocumented.
  Mitigation: lock one precedence order in spec and CLI/TUI help text.

## Migration Plan

1. Land this OpenSpec prerequisite change.
2. Implement sender precedence and relay event plumbing.
3. Implement `todos/tui/4` history viewport against locked contracts.
