## Context

The current relay host flow is single-bundle oriented. This change adds bundle
group hosting while preserving per-bundle runtime isolation (`relay.sock`,
`tmux.sock`, lock files, and reconciliation lifecycles).

The design avoids separate host-level bundle-group policy artifacts and stores
group membership with each bundle config.

## Goals

- Add `agentmux host relay --group <GROUP>` with deterministic behavior.
- Preserve `agentmux host relay <bundle-id>` compatibility.
- Keep group membership in bundle-local configuration.
- Provide machine-first startup outcomes with text rendering for operators.

## Non-Goals

- Real-time watcher reconciliation (`--watch`) in MVP.
- CLI include/exclude overrides in MVP.
- Changes to relay request/response contracts for per-bundle `list`/`chat`.

## Decisions

- Decision: Use explicit selector modes.
  - `<bundle-id>` for single-bundle host mode.
  - `--group <GROUP>` for group host mode.
  - These modes are mutually exclusive.

- Decision: Bundle groups are configured in bundle files.
  - Path: `<config-root>/bundles/<bundle-id>.toml`
  - Optional top-level key: `groups = ["..."]`
  - Why: keeps bundle ownership and grouping policy in one place and avoids
    dual-edit drift.

- Decision: Group naming convention differentiates reserved vs custom names.
  - Reserved/system names are uppercase.
  - Custom names are lowercase.
  - MVP reserved group: `ALL`.

- Decision: `ALL` is implicit.
  - `--group ALL` selects all configured bundles.
  - Bundle files do not need explicit `ALL` membership.

- Decision: Use partial-host startup for group mode.
  - Lock-held bundles are skipped and reported.
  - Hostable bundles continue startup.
  - Exit non-zero only when zero bundles successfully host.

- Decision: Canonical startup summary payload is shared by host modes.
  - `schema_version`
  - `host_mode` (`single_bundle`|`bundle_group`)
  - optional `group_name` (present in group mode)
  - `bundles[]` entries:
    - `bundle_name`
    - `outcome` (`hosted`|`skipped`|`failed`)
    - `reason_code` (nullable)
    - `reason` (nullable)
  - aggregate fields:
    - `hosted_bundle_count`
    - `skipped_bundle_count`
    - `failed_bundle_count`
    - `hosted_any`
  - lock contention encoding:
    - `outcome=skipped`
    - `reason_code=lock_held`

- Decision: No `--all` alias in MVP.
  - `--group ALL` is the canonical all-bundles selector.

## Bundle Config Draft Shape

```toml
format-version = 1
groups = ["dev", "login"]

[[sessions]]
id = "relay"
name = "Relay"
directory = "/home/me/src/WORKTREES/agentmux/relay"
coder = "codex"
```

## Group Resolution Rules

- `--group ALL`: all configured bundles.
- `--group <lowercase-name>`: bundles whose `groups` include that name.
- If a non-`ALL` group selects zero bundles, return structured
  `validation_unknown_group`.

## Trust Boundary

Group hosting remains within existing local trust assumptions:
- same-user ownership checks,
- same-host local runtime sockets,
- no new remote control surface.

## Extension Note (`--watch`)

Future watch-mode work should reuse:
- bundle `groups` resolver,
- per-bundle startup outcome model,
- canonical startup summary schema.

## Risks / Trade-offs

- Group names introduce naming-governance complexity.
  - Mitigation: explicit reserved/custom naming rules and validation.

- Group mode can hide partial failures if summaries are ignored.
  - Mitigation: per-bundle outcomes, aggregate counts, and non-zero when
    `hosted_bundle_count == 0`.
