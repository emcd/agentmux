# MCP Surface

This reference describes the operator-facing MCP tool surface exposed
by `agentmux host mcp`. The README keeps a short pointer; this doc
carries the full tool inventory and delivery-behavior notes.

## Tool inventory

The MCP server advertises:

- `help`: return tool/command help and JSON argument schemas.
- `list`: meta-tool for relay discovery; requires a `command` —
  `principals` (session listing, with `args.namespace` scoping the
  lookup), `namespaces`, `relays`, or `decisions` (the bundle's
  pending ACP choice queue).
- `look`: capture a read-only session snapshot from a target session.
- `choose`: submit an ACP-native choice decision; gated on the
  `choose` policy capability.
- `updown`: administer the associated bundle's runtime
  (`command="up"` / `command="down"`).
- `new`: register a peer principal and mint its PSK
  (`command="peer"`).
- `change`: rotate a principal's PSK (`command="psk"`).
- `raww`: write raw text directly to one target session.
- `send`: deliver to explicit targets or broadcast.

See [`src/mcp/README.md`](../../src/mcp/README.md) for detailed MCP
command semantics, validation behavior, error mapping, and module
layout.

## Delivery behavior

- No elapsed-time bound applies to a delivery's wait on a target that is
  reachable but not ready, on any transport, and there is no per-call
  operator override. What is bounded there is the queue rather than the
  wait: per-target admission quota in the `relay.toml` `[delivery]` table.
  A continuously unreachable target is bounded separately — its messages
  resolve `not_submitted` after `[delivery].unreachable-dwell-ms`.
- Pty sessions use the same look bounds as Tmux (the relay truncates
  to `mode.lines` rows). Pty grid dimensions are configured per-coder
  under `[coders.<id>.pty]` (`cols`, `rows`).
- Terminal completion is correlated out-of-band by `message_id`.

## Association resolution for `host mcp`

`host mcp` resolves bundle and session identity by tier precedence
(see [`src/runtime/association.rs`](../../src/runtime/association.rs)
and [`multi-worktree-workflow.md`](multi-worktree-workflow.md)):

- Bundle: `--bundle` > injected `AGENTMUX_BUNDLE` > effective
  `mcp.toml` > `--default-bundle`.
- Session: `--session-name` > injected `AGENTMUX_SESSION` > effective
  `mcp.toml`.

A working-directory match against declared member directories applies
as a separate inference step once the bundle resolves.

## Operational references

- Shared runtime flags and relay-host defaults:
  [operations.md](operations.md).
- TUI session identity resolution and browsing:
  [tui.md](tui.md).
- See [maintainer-configuration-guide.md](maintainer-configuration-guide.md)
  for file-by-file configuration root contents, layering, and starter
  hydration.
- Authorization policy scopes, the per-control ladder, and the home
  namespace concept: [authorization.md](authorization.md).
- Multi-worktree topology and association resolution:
  [multi-worktree-workflow.md](multi-worktree-workflow.md).
