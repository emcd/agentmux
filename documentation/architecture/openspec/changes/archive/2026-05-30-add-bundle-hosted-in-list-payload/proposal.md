# Change: Add bundle hosted flag and per-session readiness to relay list payload

## Why

Relay list payloads expose `state: up|down` derived from per-session readiness
(`ready_session_count > 0`). That field conflates lifecycle hosting intent
with operational session readiness, and the list payload exposes no
per-session liveness field, so clients cannot tell which configured sessions
are actually up.

Two genuinely distinct runtime states collapse to the same `state=down`
output today:

- `bundle up` ran, all configured sessions failed startup.
- `bundle up` was never run, or `bundle down` removed the hosted runtime.

And per-session detail (which member is ready and which is not) is
discarded after being counted into the bundle-level aggregate.

## What Changes

- Add a required additive field `ready: bool` to `ListedSession` in the
  relay list payload. Per-transport probes:
  - tmux: `resolve_active_pane_target` succeeds.
  - ACP: `acp_session_ready_for_startup` returns true.
  - ui / pubsub: always `false` (no implemented startup path).
- Add a required additive field `hosted: bool` to `ListedBundle`. Predicate:
  `hosted=true` iff at least one configured member is ready (per the
  per-member predicate above). Independent of `state`, `startup_health`,
  and `state_reason_code`.
- Per-session readiness is computed once per list request and is the
  shared source of truth for `hosted` and the existing
  `ready_session_count`-driven `state`/`startup_health` aggregates.

## Impact

- Affected specs:
  - `session-relay` (two new requirements: per-session readiness, bundle
    hosted predicate).
- Affected code:
  - `src/relay/contract.rs` (`ListedSession.ready`, `ListedBundle.hosted`).
  - `src/relay/handlers/listing.rs` (single per-member probe; populate
    both fields).
  - `src/mcp/server.rs` and `src/commands/list.rs` synthesizers set
    `ready: false` and `hosted: false` for relay-unreachable bundles.
  - `tests/integration/session_relay_list.rs` (round-trip on tmux bundle).
  - `tests/unit/relay.rs` (tmux not-ready, ACP not-ready).
  - Fake-relay fixtures across `tests/integration/cli/list.rs`,
    `tests/integration/mcp/list.rs`,
    `tests/integration/runtime_bootstrap.rs`,
    `tests/unit/relay_stream_client.rs`.
- Downstream follow-up (out of scope, separate dispatches):
  - CLI list surface display.
  - MCP list payload pass-through and tool description.
  - TUI list view consumption.

## Notes

Field shape decided as `bool` for both fields rather than enums. Both
states are binary today; an enum would invite hypothetical third states
that do not exist. Alpha breaking change is accepted if a third state
ever emerges.

Bundle-level `state` / `startup_health` aggregates are retained as
convenience summaries. Deprecating them in favor of "derive from
per-session readiness" would create avoidable downstream churn and is
explicitly out of scope.
