# Change: Add Initial TUI MVP Workbench Proposal

## Why

Operators currently coordinate through CLI and MCP surfaces only. That works
for scripted usage, but common operator workflows (recipient selection, message
compose, quick inspection, and delivery feedback) are slower and less coherent
than they need to be for daily multi-agent operations.

## What Changes

- Add a new `tui-surface` OpenSpec capability delta that defines initial TUI
  MVP behavior and boundaries.
- Define MVP workflows for:
  - recipient discovery/selection,
  - compose-and-send,
  - look/inspect snapshot viewing,
  - per-target delivery feedback.
- Lock recipient-entry semantics to explicit `To`/`Cc` fields with deterministic
  recipient identifiers and keyboard autocomplete behavior (`Tab`).
- Define a forward-compatible target identifier grammar up front:
  - local: `<session-id>` (MVP accepted),
  - qualified: `<bundle-id>/<session-id>` (reserved for future cross-bundle use).
- Keep cross-bundle delivery/inspection explicitly out of scope for MVP while
  preserving the identifier grammar for future expansion.

## Non-Goals (MVP)

- Implementing cross-bundle delivery/inspection behavior.
- Multi-relay fleet management UX (`host --group`, host orchestration).
- Historical transcript browser/search and durable archive UX.
- Authorization policy redesign.
- Rich editor features (attachments, templates, multi-buffer drafts).

## Impact

- Affected specs:
  - `tui-surface` (new capability delta)
- Affected code:
  - none in this change (planning/spec-first only)
