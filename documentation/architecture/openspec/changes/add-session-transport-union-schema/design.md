## Context

We aligned on modeling target as a coder property. Sessions already reference
coders, so target on coder gives better reuse and avoids repeated descriptors in
bundle session entries.

`TUI` is intentionally out-of-scope for this target enum because it likely
represents a separate non-coder session category.

## Goals

- Keep `[[sessions]]` as routing identity + coder association.
- Model target class on `[[coders]]`.
- Support coder target classes:
  - `tmux`
  - `acp`
- Preserve clear raw-to-validated modeling for Serde and diagnostics.
- Preserve existing bundle-membership invariants:
  - unique session IDs,
  - unique optional session names,
  - rejection of unknown coder references.
- Define ACP lifecycle selection from session identity state (`coder-session-id`)
  with fail-fast load behavior.

## Non-Goals

- TUI target/schema in this proposal.
- Runtime adapter implementation details.
- CLI/MCP API changes.

## Decision: Direct Coder Target Tables

Canonical one-of shape per coder:

- `[coders.tmux]`
- `[coders.acp]`

Exactly one table SHALL be present for each `[[coders]]` entry.

### Rationale

- Reuse: multiple sessions can reference one coder target definition.
- Ergonomics: avoids duplicating ACP/tmux descriptors on every session.
- Validation clarity: enforce one-of on coders, then validate sessions against
  coder target rules.

## Serde Modeling Pattern

Use raw-to-validated conversion:

1. Deserialize `RawCoder` with optional target tables:
   - `tmux: Option<RawTmuxTarget>`
   - `acp: Option<RawAcpTarget>`
2. Validate one-of cardinality (exactly one target table present).
3. Impute to validated `Coder { target: Target }`.
4. Deserialize `RawSession` with coder reference + optional `coder-session-id`.
5. Validate session against referenced coder target constraints and impute
   validated `Session` (non-optional linked/derived target semantics).

This keeps deserialization permissive and validation explicit with strong
errors.

## Tmux Coder Descriptor Baseline

For `[coders.tmux]`:

- required: `initial-command`
- required: `resume-command`
- optional: `prompt-regex`
- optional: `prompt-inspect-lines`
- optional: `prompt-idle-column`

## ACP Coder Descriptor Baseline

For `[coders.acp]`:

- required: `channel` (`stdio` | `http`)
- for `channel = "stdio"`:
  - required: `command` (string command template)
- for `channel = "http"`:
  - required: `url`
  - optional: `headers[]`

Session/routing constraints:

- if session includes `coder-session-id`, ACP runtime SHALL use `session/load`.
- if session omits `coder-session-id`, ACP runtime SHALL use `session/new`.
- if ACP `session/load` fails, runtime SHALL fail fast and SHALL NOT silently
  fall back to ACP `session/new` in the same operation.

This keeps identity ownership explicit and prevents accidental context forks.

## Alternatives Considered

1. Session-level target tables (`[sessions.tmux]`, `[sessions.acp]`).

- Pros: explicit per-session target config.
- Cons: duplicates shared coder/transport config; weaker reuse.

2. `target/class` discriminator table.

- Pros: explicit discriminator field.
- Cons: extra indirection and mismatch risk between class and nested tables.

3. ACP `session-mode` enum in config (`new` | `load`).

- Pros: explicit operator control.
- Cons: duplicates session identity signal already present via
  `coder-session-id`; increases configuration drift risk.

## Schema Version Strategy

This proposal uses `format-version = 2` as a clean break for coder-target
modeling. Earlier versions are validation errors under this contract.

## Risks / Trade-offs

- Clean break requires coordinated config migration.
- Session-to-coder validation becomes more central and must produce clear
  diagnostics.
- ACP runtime parity work remains follow-up implementation.
- ACP `look` parity remains out of scope in this change and requires separate
  contract work.
