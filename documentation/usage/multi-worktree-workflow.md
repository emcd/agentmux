# Multi-Worktree Workflow

This reference describes the typical topology when one agentmux
deployment serves multiple worktrees (or multiple lanes within one
worktree) through a single relay. The README keeps a short pointer;
this doc carries the topology and association-resolution rules.

## Typical topology

- one shared bundle id (for example `agentmux`),
- one relay host process serving all configured bundles through a
  single socket,
- one MCP host per worktree/session identity (`master`, `relay`,
  `mcp`, `tui`).

The relay binds one socket and serves every configured bundle;
bundle routing is determined by the `bundle_name` carried in each
client's `hello` frame. Two MCP servers running in two worktrees each
connect to the same relay and identify as different sessions of the
same bundle.

## Association resolution

`host mcp` resolves association by tier precedence; see
[`src/runtime/association.rs`](../../src/runtime/association.rs).

- bundle from `--bundle`, else the relay-injected `AGENTMUX_BUNDLE`,
  else the effective `mcp.toml`, else `--default-bundle`.
- session from `--session-name`, else the relay-injected
  `AGENTMUX_SESSION`, else the effective `mcp.toml`, else — only
  when no tier supplied one — the working directory's match against
  declared member directories.

A supplied identity which does not resolve to a known member is a
fault, not a fallback to a lower tier; presence carries operator
intent, so an unresolvable candidate must surface the typo rather
than fall into the working-directory match.

`list principals` uses the one-shot UI/user selector family rather
than the MCP chain above; see
[`src/runtime/tui_session.rs:51-65`](../../src/runtime/tui_session.rs)
and [`src/commands/list.rs:24-50`](../../src/commands/list.rs):

- the listing scope comes from `--namespace` (a bundle name, `GLOBAL`
  for relay-wide `@GLOBAL` principals, or `*` for fan-out).
- the requester identity resolves as: the concrete `--namespace`
  value (used as the bundle hint when it names a bundle, which it
  must to seed the hint; `--namespace = GLOBAL` and `--namespace = *`
  deliberately do not) > `default-bundle` from `ui.toml`;
  explicit `--as-session` > `default-session` from `users.toml`.

`send` and `tui` use the same one-shot UI/user selector family:

- `--bundle`, else `default-bundle` from `ui.toml`,
- `--as-session`, else `default-session` from `users.toml`,
- one-shot `send` fails fast when the bundle is missing or unknown;
  interactive `tui` falls back to the first available bundle (or an
  empty browsing context).

## TUI session identity resolution

- `--as-session` selector
- `default-session` from active `users.toml`
- no association fallback for TUI/send

## Bundle updown

`updown` (CLI: `agentmux up` / `agentmux down`) administers a
bundle's runtime. The relay authorizes the calling principal against
the `updown` policy control, which is deny by default. Configured
operators carry the grant; a session whose policy does not enable
`updown` cannot bring bundles up or down and surfaces
`authorization_forbidden`.

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
- MCP tool inventory and delivery behavior:
  [mcp-surface.md](mcp-surface.md).
