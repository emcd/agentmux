## Context

`add-session-transport-union-schema` established ACP coder configuration and a
first ACP delivery spike. The next step is to harden relay behavior so ACP
send semantics are deterministic under retries, restarts, and capability
variance.

## Goals

- Define one durable ownership model for ACP `sessionId` continuity.
- Keep load/new selection deterministic and fail-fast.
- Define stable ACP outcome mapping for relay send responses.
- Avoid transport-model leakage from tmux quiescence semantics into ACP.

## Non-Goals

- ACP `look` parity behavior.
- New MCP/CLI request parameters.
- Cross-bundle policy/authorization redesign.

## Decisions

- Decision: ACP lifecycle selection precedence for send is:
  1. configured session `coder-session-id`
  2. relay-persisted ACP session id for that bundle session
  3. otherwise `session/new`

- Decision: `session/load` failure remains fail-fast and MUST NOT fall back to
  `session/new` in the same send operation.

- Decision: ACP capability checks are explicit:
  - `initialize` must succeed before ACP lifecycle calls,
  - load path requires advertised load-session capability,
  - prompt path requires prompt-session capability.

- Decision: ACP transport uses turn-wait semantics:
  - request-level `quiescence_timeout_ms` is interpreted as ACP turn-wait
    timeout,
  - pane-quiescence gates do not apply to ACP sends.

- Decision: Relay maps ACP terminal stop reasons to canonical delivery
  outcomes with stable reason codes.

## Risks / Trade-offs

- Trade-off: persisting ACP session ids adds runtime state coupling, but avoids
  accidental session forks after process restart.
- Risk: capability variance across ACP agents can cause surprise failures.
  Mitigation: explicit capability-gating errors and tests.
- Risk: timeout behavior ambiguity between transports.
  Mitigation: transport-specific timeout wording locked in spec.

## Migration Plan

1. Land this proposal and implementation.
2. Add integration tests for persistence and failure semantics.
3. Follow with ACP look behavior proposal (unsupported or synthesized model).
