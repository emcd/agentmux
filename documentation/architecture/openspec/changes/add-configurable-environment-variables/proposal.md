# Change: Configurable environment variables for coders, bundles, and sessions

## Why

Spawned coder sessions cannot declare arbitrary environment variables except
through the ACP-specific `[coders.<id>.acp].environment` subtable, which only
applies to ACP command-spawn targets. The motivating case is
`OPENCODE_ENABLE_EXA=1`, which the Opencode websearch tool needs; the operator
currently carries it as a stopgap in the relay systemd unit. Tmux- and
Pty-backed coders have no way to declare environment at all, and there is no
bundle- or session-level environment surface. This closes
`agents-common:todos/template/9` (enable the Opencode websearch tool).

## What Changes

- Add an optional `environment` array (reusing `NameValueEntry`, `{ name,
  value }`) at three declaration sites:
  - `[coders.<id>].environment` in `coders.toml` (applies across every
    transport the coder uses).
  - top-level `environment` in a per-bundle `bundles/<id>.toml`.
  - per-session `[[sessions]].environment` in a per-bundle file.
- Resolve the three levels into a single merged environment per session at
  configuration-load time. Precedence is per-variable, most-specific-wins:
  **session > bundle > coder**. Non-colliding variables union across levels.
- Apply the merged environment to the spawned child on all three transports:
  Tmux (`new-session -e KEY=VALUE`), Pty (`Command::env`), and ACP
  command-spawn (`Command::env`, already the mechanism for the old ACP-only
  field). For non-spawning targets (ACP `http` channel, `ui`/`pubsub`
  markers) a declared environment is inert.
- **BREAKING**: relocate the existing environment surface from the
  ACP-specific `[coders.<id>.acp].environment` (raw) /
  `AcpTargetConfiguration.environment` (resolved) up to the coder level
  (`[coders.<id>].environment` / merged `BundleMember.environment`), and drop
  the ACP-specific copy. ACP `headers` stay ACP-only. Existing configs using
  the old `[coders.<id>.acp].environment` key fail the raw loader's
  `deny_unknown_fields` check and must move the key up one level. Permitted
  under alpha-defaults (no back-compat); flagged explicitly so this reads as a
  schema move, not a pure addition.

## Impact

- Affected specs: `session-relay` (new ADDED requirements for coder, bundle,
  and session environment plus the merge/precedence rule — orthogonal to the
  coder-target-descriptor requirement so this stays archive-order-independent
  from the still-unarchived `add-pty-transport` change).
- Affected code:
  - `src/configuration/{raw,types,targets,fields,loaders}.rs` — schema,
    validation, resolution + merge helper (merge lands in `loaders.rs`, where
    the bundle, session, and resolved coder are all in scope).
  - `src/acp/persistent_runtime.rs` — the ACP field consumer
    (`initialize_persistent_acp_worker_runtime` reads `&acp.environment`);
    re-source from the merged `BundleMember.environment`. **Compile-critical:
    removing the ACP-specific field (task 1.3) breaks this call site.**
  - `src/pty/transport.rs` — apply merged environment on spawn (Pty-Specialist
    review).
  - `src/tmux/pane.rs` — apply merged environment via `new-session -e`.
  - Tests: rewrite `tests/unit/config/coder.rs`
    (`loads_acp_coder_with_environment`) and the
    `tests/integration/acp/helpers.rs` fixture to the new
    `[coders.<id>].environment` key.
- Cross-lane: the `src/pty/transport.rs` hunk is reviewed by the Pty
  Specialist (review-only; BE owns the full slice).
