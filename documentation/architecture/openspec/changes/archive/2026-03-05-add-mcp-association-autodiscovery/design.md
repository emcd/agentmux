## Context

`tmuxmux` currently has pieces of sender inference, but association logic is
not yet strict or complete for real multi-worktree workflows.

Target workflow:

- Main clone: `~/src/tmuxmux` (bundle name `tmuxmux`)
- Worktree: `~/src/WORKTREES/tmuxmux/relay` (sender session `relay`)
- Worktree: `~/src/WORKTREES/tmuxmux/tui` (sender session `tui`)

MCP startup should infer those identities without requiring operators to pass
flags for normal development.

## Goals / Non-Goals

- Goals:
  - Auto-discover bundle and sender for common Git clone/worktree layouts.
  - Keep deterministic precedence for explicit overrides.
  - Fail fast when discovery cannot produce a safe unique association.
  - Support local testing/cross-project coordination via file-based overrides.
- Non-Goals:
  - Runtime mutation of bundle membership through MCP tools.
  - Cross-host discovery.
  - Persistent global daemon for association lookup.

## Decisions

- Decision: resolve association with strict precedence.
  - Order:
    1. CLI explicit flags (`--bundle-name`, `--session-name`)
    2. Local override file (`.auxiliary/configuration/tmuxmux/overrides/mcp.toml`)
    3. Auto-discovery heuristics
  - Rationale: explicit operator intent should always win.

- Decision: bundle auto-discovery uses Git common-dir identity.
  - In Git repositories:
    - derive bundle from basename of parent directory of `git common-dir`
    - e.g., `git common-dir = /home/me/src/tmuxmux/.git` -> bundle `tmuxmux`
  - Outside Git:
    - fallback bundle candidate is basename of current working directory
  - Rationale: this distinguishes shared project identity from worktree name.

- Decision: sender auto-discovery uses worktree root identity.
  - In Git repositories:
    - derive sender from basename of `git rev-parse --show-toplevel`
  - Outside Git:
    - fallback sender candidate is basename of current working directory
  - Rationale: worktree basename naturally encodes role/session labels.

- Decision: startup is strict on missing/ambiguous association.
  - If discovered or overridden bundle does not map to existing bundle config:
    - fail startup with structured `validation_unknown_bundle`
  - If sender is missing, ambiguous, or not a bundle member:
    - fail startup with structured `validation_unknown_sender`
  - Rationale: avoid silently routing from wrong identities.

- Decision: support local override file for controlled exceptions.
  - File:
    - `<workspace_root>/.auxiliary/configuration/tmuxmux/overrides/mcp.toml`
  - VCS posture:
    - `.auxiliary/configuration/tmuxmux/overrides/` is Git-ignored for
      per-worktree/private local overrides.
  - Shape:
    - single optional `bundle_name` override
    - single optional `session_name` override
    - optional override of config root path for cross-project bundle lookup
  - Rationale: keep MVP override simple and explicit.

## Risks / Trade-offs

- Git metadata probing introduces extra startup branching and failure modes.
- Fallback to current directory basename can be wrong in ad-hoc directories.
- Override files add flexibility but can hide accidental misconfiguration.

## Migration Plan

1. Implement discovery helpers for Git and non-Git contexts.
2. Add override file parsing and precedence resolver.
3. Harden startup validation and structured error returns.
4. Add unit/integration tests for clone/worktree/non-Git/override paths.
5. Document startup precedence and troubleshooting.
