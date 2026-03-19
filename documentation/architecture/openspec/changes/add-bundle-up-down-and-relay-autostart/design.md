## Context

Current contracts bind bundle hosting selection tightly to `agentmux host relay`
selector flags. This is workable for manual use but suboptimal for service
startup and explicit operational reconciliation.

## Goals

- Separate relay process lifecycle from bundle hosting lifecycle.
- Support one-command relay startup with autostart eligibility.
- Keep deterministic selector, error, and summary payload semantics.
- Preserve same trust boundary and existing group naming rules.

## Non-Goals

- No cross-bundle authorization redesign.
- No new remote control surface.
- No deprecation/removal of existing selector-based host modes in this change.

## Decisions

- Decision: command shape for MVP is `agentmux up` / `agentmux down`
  (no `bundle` namespace alias in this change).
- Decision: `agentmux host relay` is no-selector only.
- Decision: autostart defaults to enabled; `--no-autostart` toggles
  process-only startup behavior.
- Decision: host-relay success is tied to process startup,
  not hosted bundle count.
- Decision: per-bundle autostart eligibility uses optional top-level
  `autostart` with default false.
- Decision: bundle groups remain supported for `up/down` selector resolution
  only.
- Decision: `up/down` require a running relay and return `relay_unavailable`
  when relay is not reachable.
- Decision: `up/down` are idempotent and surface no-op transitions as
  `outcome=skipped` with canonical reason codes.
- Decision: `up/down` lifecycle summaries are machine-readable and deterministic
  with declaration-order bundle entries.

## Risks / Trade-offs

- Removing `host relay` selectors simplifies process startup and avoids command
  overlap, but changes existing explicit host-selector workflow.
- `--no-autostart` introduces one additional flag path but keeps process-only
  startup explicit and scriptable.

## Migration Plan

1. Land spec deltas for CLI/runtime/session-relay.
2. Implement parser/routing updates for new command modes.
3. Implement relay lifecycle control operations and summaries.
4. Add integration tests for no-selector autostart, process-only startup, and
   idempotent up/down transitions.
