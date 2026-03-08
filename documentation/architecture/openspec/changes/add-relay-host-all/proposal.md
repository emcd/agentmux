# Change: Add relay bundle-group hosting via `agentmux host relay --group`

## Why

Operators currently host relay for one bundle at a time using
`agentmux host relay <bundle-id>`. Multi-bundle startup is useful, but a
separate host-level policy file introduces dual-edit and rename-drift risk.

Bundle-group membership should live with each bundle definition, and relay host
selection should support bundle groups directly.

## What Changes

- Extend relay host selector modes to support:
  - `agentmux host relay <bundle-id>`
  - `agentmux host relay --group <GROUP>`
- Keep selector modes mutually exclusive.
- Add optional top-level `groups` in each bundle file
  (`bundles/<bundle-id>.toml`).
- Define naming convention for groups:
  - reserved/system groups: uppercase (MVP reserves `ALL`)
  - custom groups: lowercase
- Define `ALL` as a special implicit group representing all configured bundles.
- Define group startup contention behavior:
  - partial host (skip lock-held bundles, continue others)
  - non-zero exit only when zero bundles are successfully hosted
- Define canonical machine startup summary payload for host modes, including
  aggregate counts and per-bundle outcomes.

## Non-Goals (MVP)

- `--include-bundle` and `--exclude-bundle` CLI overrides
- Dynamic discovery/watch behavior (`agentmux host relay --watch`)
- Cross-user or cross-host relay control beyond current local runtime model

## Migration Expectations

- Existing operators using `agentmux host relay <bundle-id>` require no changes.
- Operators adopting `--group <GROUP>` define membership in bundle files via
  optional `groups`.
- `--group ALL` works without per-bundle edits because `ALL` is implicit.

## Impact

- Affected specs:
  - `cli-surface`
  - `runtime-bootstrap`
  - `session-relay`
- Affected code:
  - `src/commands.rs` relay host argument parsing and startup flow
  - configuration loader for optional bundle `groups`
  - group selection resolver and startup orchestration
  - relay startup reporting and inscriptions
  - CLI integration/unit tests for selector modes and startup outcomes
