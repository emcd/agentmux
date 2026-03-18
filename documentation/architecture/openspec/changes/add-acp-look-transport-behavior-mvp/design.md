## Context

`look` is defined as ordered pane snapshot lines. This is a tmux-native concept.
ACP prompt-turn streams do not currently provide a canonical equivalent snapshot
contract in agentmux.

## Goals

- Remove ambiguity by locking ACP `look` behavior for MVP.
- Keep adapter behavior consistent across relay, MCP, and CLI surfaces.
- Preserve current tmux `look` semantics and payload shape.

## Non-Goals

- Designing rolling ACP snapshots for `look`.
- Introducing additional look tools/commands.

## Decisions

- Decision: ACP-target `look` is unsupported in MVP.
- Decision: Relay returns stable validation error code
  `validation_unsupported_transport` for ACP-target look requests.
- Decision: MCP and CLI propagate relay-authored rejection semantics without
  adapter-specific reinterpretation.

## Risks / Trade-offs

- Trade-off: ACP sessions lack immediate look parity in MVP.
- Benefit: avoids under-specified synthetic history behavior and memory-policy
  complexity.

## Migration Plan

1. Land this explicit unsupported contract.
2. If ACP look parity is later required, open a dedicated proposal for bounded
   synthesized snapshots with explicit retention/ordering contract.
