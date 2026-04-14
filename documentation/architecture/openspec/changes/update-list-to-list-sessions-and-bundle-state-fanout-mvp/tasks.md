## 1. Contract Design

- [ ] 1.1 Relock list naming surfaces to `agentmux list sessions` (CLI) and `list.sessions` (MCP).
- [ ] 1.2 Lock canonical single-bundle payload schema using `bundle.id` + `bundle.state` + `bundle.sessions[]`.
- [ ] 1.3 Lock session entry schema and vocabulary:
  - configured members scope
  - `transport` in (`tmux`, `acp`)
- [ ] 1.4 Lock down-state reason taxonomy and evidence mapping:
  - `not_started`
  - `relay_unavailable`
- [ ] 1.5 Lock relay list request scope to single-bundle only (no all-bundles relay selector in MVP).

## 2. All-Mode Fanout Semantics

- [ ] 2.1 Lock adapter-owned all-mode selectors:
  - CLI `--all`
  - MCP `all=true`
- [ ] 2.2 Lock selector mutual exclusivity (`--bundle` vs `--all`; `bundle_name` vs `all=true`).
- [ ] 2.3 Lock deterministic all-mode ordering (lexicographic bundle id).
- [ ] 2.4 Lock all-mode fail-fast authorization behavior on first `authorization_forbidden`.
- [ ] 2.5 Lock all-mode error surface as non-aggregate on fail-fast deny.

## 3. Unreachable Relay Fallback Contract

- [ ] 3.1 Lock canonical fallback synthesis behavior when bundle relay is unreachable.
- [ ] 3.2 Lock fallback authorization posture:
  - home-bundle synthesis allowed
  - non-home unreachable target returns `relay_unavailable`
- [ ] 3.3 Lock single-bundle down-path canonical payload requirement when fallback synthesis is authorized.

## 4. Migration and Follow-up Implementation

- [ ] 4.1 Mark legacy `list` CLI/MCP surfaces as removed in this pre-MVP change.
- [ ] 4.2 Add implementation follow-up items for relay request/response updates.
- [ ] 4.3 Add implementation follow-up items for CLI parser/output updates.
- [ ] 4.4 Add implementation follow-up items for MCP tool registration/handler updates.
- [ ] 4.5 Add implementation follow-up items for adapter fanout/fallback tests.

## 5. Validation

- [ ] 5.1 Run `openspec validate update-list-to-list-sessions-and-bundle-state-fanout-mvp --strict`.
