# Change: Add ACP Permission-Request UI-Mediated Handling (Alpha)

## Why

ACP `session/request_permission` currently lacks a locked relay-authoritative
contract for queueing, operator decisioning, and deterministic enforcement.
Without a canonical model, implementations can drift on trust boundaries,
timeout behavior, and sender-visible outcomes.

Primary reference standard:
- ACP Tool Calls: https://agentclientprotocol.com/protocol/tool-calls.md

Implementer directive:
- Before coding or reviewing this change, implementers SHOULD read the ACP Tool
  Calls spec end-to-end, especially `session/request_permission` request/response
  semantics and permission option kinds. This change is intentionally aligned
  to that standard and not an independent local protocol.

## What Changes

- Add relay policy capability `grant` for permission-decision authority.
- Lock UI-mediated decisioning:
  - decision submitter must be `client_class=ui`,
  - decision actor identity is association-derived (non-spoofable payload).
- Add deterministic same-bundle queue/routing model for permission requests:
  - bounded queue with canonical overflow behavior,
  - FIFO replay/snapshot on authorized UI connect,
  - durable pending-state restoration across restart.
- Lock non-expiring pending semantics for alpha:
  - permission requests remain pending until explicit decision or hard terminal
    conditions.
- Preserve ACP permission-option fidelity:
  - relay and UI surfaces carry ACP option metadata through permission events,
  - decision actions use ACP-native outcomes (`selected` or `cancelled`),
  - `selected` decisions MUST include explicit `option_id`,
  - relay option-selection heuristics are not allowed in alpha.
- Lock canonical lifecycle machine events and required correlation keys.
- Lock deterministic mapping from permission outcomes to:
  - ACP selected/cancelled behavior,
  - sender-visible terminal outcome/reason_code semantics.
- Add TUI-facing contract for pending visibility and actionable decision flows
  using ACP-native option selection in a session-scoped workflow.

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
