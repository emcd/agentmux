# Change: Harden ACP send runtime semantics for MVP

## Why

The current ACP path is a working spike but lacks fully locked runtime
contracts for session-id continuity, capability gating, and deterministic
outcome mapping. Without those contracts, ACP behavior can drift across relay,
MCP, and CLI surfaces during implementation.

## What Changes

- Define durable ACP session-id ownership semantics in relay runtime state.
- Lock lifecycle selection precedence for ACP send:
  - config `coder-session-id`
  - runtime persisted ACP session id
  - otherwise `session/new`
- Keep fail-fast load semantics: no fallback from `session/load` to
  `session/new` in the same operation.
- Lock ACP capability gating for `initialize`, `session/new`, `session/load`,
  and `session/prompt`.
- Lock ACP stop-reason and timeout mapping into relay delivery outcomes with
  stable reason codes.
- Lock transport-specific timeout semantics for ACP send (turn-wait based,
  not pane-quiescence based).

## Non-Goals

- Defining ACP `look` behavior (tracked separately).
- Redesigning MCP/CLI send request shape.
- Introducing ACP HTTP adapter implementation in this proposal.

## Impact

- Affected specs:
  - `session-relay`
- Expected follow-up implementation touchpoints:
  - relay ACP state persistence path
  - relay ACP delivery mapping
  - ACP integration tests for lifecycle, timeout, and stop-reason mapping
