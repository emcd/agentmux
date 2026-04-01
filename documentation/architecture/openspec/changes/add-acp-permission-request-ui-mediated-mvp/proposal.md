# Change: Add ACP Permission-Request UI-Mediated Handling (MVP)

## Why

ACP `session/request_permission` currently lacks a locked relay-authoritative
contract for queueing, operator decisioning, and deterministic enforcement.
Without a canonical model, implementations can drift on trust boundaries,
timeout behavior, and sender-visible outcomes.

## What Changes

- Add relay policy capability `grant` for permission-decision authority.
- Lock UI-mediated decisioning:
  - decision submitter must be `client_class=ui`,
  - decision actor identity is association-derived (non-spoofable payload).
- Add deterministic same-bundle queue/routing model for permission requests:
  - bounded queue with canonical overflow behavior,
  - FIFO replay/snapshot on authorized UI connect,
  - durable pending-state restoration across restart.
- Lock non-expiring pending semantics in MVP:
  - permission requests remain pending until explicit decision or hard terminal
    conditions.
- Lock canonical lifecycle machine events and required correlation keys.
- Lock deterministic mapping from permission outcomes to:
  - ACP allow/deny/abort behavior,
  - sender-visible terminal outcome/reason_code semantics.
- Add TUI-facing contract for pending visibility and approve/deny actions.

## Impact

- Affected specs:
  - `session-relay`
  - `tui-surface`
- Affected code (implementation follow-up):
  - ACP worker queueing and permission state handling
  - policy evaluation path for `grant`
  - relay stream/event emission for permission lifecycle
  - TUI pending permission rendering and decision action dispatch
  - integration tests for queue/replay/pending-lifecycle/decision mapping
