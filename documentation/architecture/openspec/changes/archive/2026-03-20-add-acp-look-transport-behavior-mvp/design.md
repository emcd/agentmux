## Context

`look` is defined as ordered snapshot lines. Tmux already satisfies this via
pane capture. ACP sends now process `session/update` events during prompt turns,
which provides a transport-native signal we can persist for deterministic ACP
look behavior.

## Goals

- Provide ACP look parity with deterministic snapshot semantics.
- Keep adapter behavior consistent across relay, MCP, and CLI surfaces.
- Preserve current tmux `look` semantics and payload shape.

## Non-Goals

- Adding cross-bundle ACP look support.
- Designing ACP HTTP look behavior.
- Introducing additional look tools/commands.

## Decisions

- Decision: Relay ingests non-empty text lines from ACP `session/update` payloads
  during ACP prompt turns and appends them to per-session ACP look state.
- Decision: ACP look state is persisted under runtime state with deterministic
  bounded retention:
  - maximum retained snapshot lines: 1000
  - eviction policy: oldest-first when exceeding cap
- Decision: ACP-target `look` returns retained snapshot tail lines based on
  requested `lines`.
- Decision: ACP look response ordering is oldest -> newest.
- Decision: When no ACP look state exists (or retained snapshot is empty),
  relay returns success with `snapshot_lines = []`.
- Decision: MCP and CLI propagate relay-authored ACP look payloads unchanged.

## Risks / Trade-offs

- Trade-off: snapshot reflects streamed updates, not a full terminal frame.
- Benefit: deterministic bounded memory/state behavior and immediate ACP look
  parity for smoke testing.

## Migration Plan

1. Replace unsupported ACP look contract with bounded snapshot contract.
2. Update relay, MCP, and CLI tests to assert success-path snapshot behavior.
3. Validate strict OpenSpec contract after delta updates.
