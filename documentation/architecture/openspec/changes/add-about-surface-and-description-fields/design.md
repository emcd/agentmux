## Context

`agentmux` now supports richer runtime topology and policy controls, but there is
no dedicated introspection surface for bundle/session intent metadata. Teams are
relying on external docs that may drift from active runtime config.

## Goals

- Add one read-only `about` contract across relay, CLI, and MCP.
- Add optional bundle/session `description` fields in bundle config.
- Keep MVP scope same-bundle only.
- Reuse existing authorization capability mapping (`list.read`) for MVP.
- Lock deterministic machine-readable schema and selector/error behavior.

## Non-Goals

- No authorization model redesign.
- No cross-bundle about support in MVP.
- No mutable write/update surface for descriptions.

## Decisions

- Decision: configuration naming is `description` (not `summary`).
- Decision: description normalization/validation is deterministic:
  - trim leading/trailing whitespace,
  - reject whitespace-only values with `validation_invalid_description`,
  - preserve internal newlines,
  - enforce max length after trim:
    - bundle `description` <= 2048 UTF-8 characters,
    - session `description` <= 512 UTF-8 characters.
- Decision: about auth reuses `list.read`; deny path remains
  `authorization_forbidden` with canonical details schema.
- Decision: about remains same-bundle only in MVP. Non-home bundle selectors
  return `validation_cross_bundle_unsupported`.
- Decision: response shape is exact and stable across CLI machine output and MCP
  payload:
  - top-level: `schema_version`, `bundle_name`, `bundle_description`,
    `sessions[]`
  - session item: `session_id`, `session_name`, `description`
  - nullable optional fields represented as explicit null, not omitted
  - `sessions[]` ordering follows bundle config declaration order
  - session selector returns exactly one item (or validation error)

## Risks / Trade-offs

- Reusing `list.read` for `about` may couple future policy granularity for
  metadata visibility. This is acceptable for MVP simplicity and can be split
  later if needed.
- Explicit null serialization constrains adapter flexibility but avoids
  cross-surface drift.

## Migration Plan

1. Land OpenSpec deltas for relay + CLI + MCP surfaces.
2. Implement config parser updates for `description` fields and validation.
3. Implement relay about operation, then CLI/MCP adapters.
4. Add tests for selector failures, auth-deny passthrough, ordering, and null
   serialization parity.
