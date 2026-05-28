# Change: Add bundle hosted flag to relay list payload

## Why

Relay list payloads expose `state: up|down` derived from per-session readiness
(`ready_session_count > 0`). That field conflates lifecycle hosting intent
with operational session readiness. Two genuinely distinct runtime states
collapse to the same `state=down` output:

- `bundle up` ran, all configured sessions failed startup.
- `bundle up` was never run, or `bundle down` removed the hosted runtime.

Clients (CLI, MCP, TUI list surfaces) cannot tell from the list payload
whether the bundle is currently hosted. Today the only way to detect hosting
state is to issue a `bundle up`/`down` transition and inspect the
`outcome=skipped` reason code, which is a transition with side effects.

## What Changes

- Add a required additive field `hosted: bool` to `ListedBundle` in the relay
  list payload.
- `hosted` SHALL be derived (probed) on each list request from runtime
  artifacts and SHALL be independent of `state` (`up|down`) and
  `startup_health`.
- Hosting predicate:
  - For bundles with at least one configured tmux member: `hosted=true` iff
    at least one configured tmux member has an agentmux-owned tmux session
    present.
  - For bundles with zero configured tmux members: `hosted=true` always
    (matches existing idempotent `bundle up` no-op behavior).
- `state_reason_code` semantics remain orthogonal to `hosted`: reason still
  describes `state` and is not suppressed when `hosted=false`.

## Impact

- Affected specs:
  - `session-relay` (new requirement under Bundle Startup Health Model
    section)
- Affected code (implementation in this change):
  - `src/relay/contract.rs` (`ListedBundle` field add)
  - `src/relay/handlers/listing.rs` (probe + populate)
  - `tests/unit/relay.rs` (regression coverage for hosted-up,
    hosted-down-with-failed-sessions, unhosted)
  - `tests/integration/cli/list.rs` (fake-relay fixtures need new field
    populated; CLI surface text changes are downstream and not in scope)
- Downstream follow-up (out of scope, separate proposals/dispatches):
  - CLI list surface display
  - MCP list payload pass-through and tool description
  - TUI list view consumption

## Notes

Field shape decided as a `bool` rather than an enum. The state is binary
today (hosted or not); an enum would invite hypothetical third states that
do not exist. Alpha breaking change is accepted if a third state ever
emerges.
