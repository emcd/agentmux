## Context

The current operator experience is contract-capable (`list`, `send`, `look`)
but split across CLI invocations and MCP tool calls. We need an initial TUI
surface that composes those existing contracts into a faster operational loop
without introducing new transport behavior in MVP.

## Goals

- Define a compact, implementation-ready TUI MVP scope.
- Reuse current contract surfaces:
  - relay operations (`list`, `chat`, `look`),
  - CLI/MCP naming (`list`, `send`, `look`),
  - stable error taxonomy.
- Support deterministic recipient addressing with low-friction keyboard entry.
- Prepare identifier design for future cross-bundle workflows without enabling
  cross-bundle behavior in MVP.

## Non-Goals

- New transport or protocol contracts.
- Cross-bundle implementation in this change.
- Historical transcript/archive features.
- Authorization model redesign.

## Decisions

- Decision: MVP compose UX uses explicit recipient fields:
  - `To`
  - `Cc`
  The canonical send target state is selected recipient IDs, not free-form text
  parsing.

- Decision: recipient entry supports inline completion:
  - `Tab` completes from known recipient identities in current bundle context.

- Decision: recipient discovery supports an overlay picker opened from keyboard
  shortcut (default `F2`) to speed selection and reduce entry errors.

- Decision: target identifier grammar is forward-compatible from day one:
  - local identifier: `<session-id>`
  - qualified identifier: `<bundle-id>/<session-id>` (reserved in MVP)

- Decision: MVP keeps scope single-bundle for delivery/inspection behavior.
  Qualified identifiers that imply cross-bundle scope are rejected with existing
  unsupported-scope validation behavior.

- Decision: TUI user-visible verbs and labels align with existing public surface:
  - `list` for recipient discovery
  - `send` for delivery
  - `look` for read-only snapshot inspection

- Decision: implementation stack direction (non-normative for spec):
  - `ratatui` + `crossterm` with existing project Rust patterns.

## Risks / Trade-offs

- Trade-off: explicit `To`/`Cc` model is more structured than free-form mention
  text, but it improves determinism and error handling.
- Risk: users may expect `@mention` compose behavior immediately.
  Mitigation: preserve extension path after deterministic field UX is stable.

## Migration Plan

1. Lock MVP behavior in OpenSpec (`tui-surface`).
2. Align coordinator/human review on scope and non-goals.
3. Start implementation change(s) after review lock.
