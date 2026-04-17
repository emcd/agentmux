## 1. Contract Design

- [x] 1.1 Relock list naming surfaces to `agentmux list sessions` (CLI) and `list.sessions` (MCP).
- [x] 1.2 Lock canonical single-bundle payload schema using `bundle.id` + `bundle.state` + `bundle.sessions[]`.
- [x] 1.3 Lock session entry schema and vocabulary:
  - configured members scope
  - `transport` in (`tmux`, `acp`)
- [x] 1.4 Lock down-state reason taxonomy and evidence mapping:
  - `not_started`
  - `relay_unavailable`
- [x] 1.5 Lock relay list request scope to single-bundle only (no all-bundles relay selector in MVP).

## 2. All-Mode Fanout Semantics

- [x] 2.1 Lock adapter-owned all-mode selectors:
  - CLI `--all`
  - MCP `all=true`
- [x] 2.2 Lock selector mutual exclusivity (`--bundle` vs `--all`; `bundle_name` vs `all=true`).
- [x] 2.3 Lock deterministic all-mode ordering (lexicographic bundle id).
- [x] 2.4 Lock all-mode fail-fast authorization behavior on first `authorization_forbidden`.
- [x] 2.5 Lock all-mode error surface as non-aggregate on fail-fast deny.

## 3. Unreachable Relay Fallback Contract

- [x] 3.1 Lock canonical fallback synthesis behavior when bundle relay is unreachable.
- [x] 3.2 Lock fallback authorization posture:
  - home-bundle synthesis allowed
  - non-home unreachable target returns `relay_unavailable`
- [x] 3.3 Lock single-bundle down-path canonical payload requirement when fallback synthesis is authorized.

## 4. Migration and Follow-up Implementation

- [x] 4.1 Mark legacy `list` CLI/MCP surfaces as removed in this pre-MVP change.
- [x] 4.2 Add implementation follow-up items for relay request/response updates.
- [x] 4.3 Add implementation follow-up items for CLI parser/output updates.
- [x] 4.4 Add implementation follow-up items for MCP tool registration/handler updates.
- [x] 4.5 Add implementation follow-up items for adapter fanout/fallback tests.

## 5. Validation

- [x] 5.1 Run `openspec validate update-list-to-list-sessions-and-bundle-state-fanout-mvp --strict`.
