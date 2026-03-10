## Context

The current session schema assumes tmux as the only transport. ACP feasibility
work showed we can support ACP as an alternative, but we need a stable and
explicit schema shape first.

The coordinator requested a schema centered on `[[sessions]].[transport]` and
explicit comparison against other TOML encodings before implementation.

## Goals

- Keep `[[sessions]]` as the canonical routing identity source.
- Introduce a transport union that supports:
  - `tmux` (current behavior)
  - `acp` (future alternative)
- Preserve straightforward migration from tmux-only bundles.
- Keep the schema strict and machine-validatable.

## Non-Goals

- Defining complete ACP runtime state machine behavior.
- Implementing relay adapter logic.
- Changing CLI/MCP API surfaces in this change.

## Decision: Tagged Per-Session Transport Descriptor

Use a per-session tagged descriptor:

- `[sessions.transport]`
  - `kind = "tmux" | "acp"`
- Optional nested ACP descriptor:
  - `[sessions.transport.acp]`

In `format-version = 2`, omitted `sessions.transport` defaults to tmux.

### Rationale

- Co-locates transport details with the session they affect.
- Avoids indirection when resolving delivery targets.
- Preserves stable `session.id` routing semantics across transport types.
- Supports mixed bundles where some sessions are tmux and others ACP.

## Alternatives Considered

1. Top-level transport registry (`[[transports]]`) with per-session references.

- Pros: deduplicates repeated transport definitions.
- Cons: cross-reference complexity, weaker local readability, harder errors.

2. Separate transport-specific session arrays (`[[tmux.sessions]]`,
`[[acp.sessions]]`).

- Pros: explicit separation.
- Cons: breaks existing `[[sessions]]` model and complicates migration.

3. Flat prefixed keys (`transport-kind`, `transport-command`, ...).

- Pros: simpler initial parser changes.
- Cons: poor extensibility and rapidly growing key surface.

## ACP Descriptor Baseline

For `kind = "acp"`:

- required: `transport` (`stdio` | `http`)
- required: `session_mode` (`new` | `load`)
- required-if: `session_id` when `session_mode = "load"`
- for `transport = "stdio"`:
  - required: `command`
  - optional: `args`, `env[]`
- for `transport = "http"`:
  - required: `url`
  - optional: `headers[]`

## Migration Strategy

1. Preserve `format-version = 1` for legacy tmux bundles.
2. Introduce `format-version = 2` for transport-aware bundles.
3. In v2, default omitted transport to tmux for minimal migration friction.
4. Keep validation fail-closed for unknown kinds and incomplete ACP
   descriptors.

## Risks / Trade-offs

- Mixed-transport bundles add complexity to relay behavior and diagnostics.
- ACP feature parity with tmux-specific behaviors (snapshot/quiescence) remains
  future implementation risk.
- Versioned schema support increases validation matrix size.
