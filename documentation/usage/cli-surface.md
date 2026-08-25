# CLI Surface

This reference describes the operator-facing command-line interface. The
README keeps a short pointer plus the essential quick-start path; this
doc carries the full surface.

## Command summary

```text
agentmux host relay [--no-autostart] [--require-credentials] [--no-watch]
agentmux host mcp [--bundle NAME] [--default-bundle NAME] [--session-name NAME]
agentmux up (<bundle-id> | --group GROUP)
agentmux down (<bundle-id> | --group GROUP)
agentmux list principals [--namespace NAME|GLOBAL|*] [--as-session NAME] [--json]
agentmux look <target-session> [--bundle NAME] [--as-session NAME] [--lines N]
agentmux raww <target-session> --text TEXT [--no-enter] [--bundle NAME] [--as-session NAME] [--json]
agentmux new peer <principal_id> [--scope SCOPE] [--output PATH | --write-config] [--bundle NAME] [--as-session NAME] [--json]
agentmux change psk <principal_id> [--output PATH | --write-config] [--bundle NAME] [--as-session NAME] [--json]
agentmux drop peer <principal_id> [--bundle NAME] [--as-session NAME] [--json]
agentmux check configuration [<bundle-id>] [-q|--quiet]
agentmux tui [--bundle NAME] [--as-session NAME] [--lines N]
agentmux send (--target NAME ... | --broadcast) [--message TEXT] [--request-id ID] [--bundle NAME] [--as-session NAME] [--json]
```

Each command also accepts the shared runtime flags
(`--configuration-directory PATH`, `--state-directory PATH`,
`--inscriptions-directory PATH` / `--logs-directory PATH`); see
[operations.md](operations.md) for resolution tiers and the
[maintainer configuration guide](maintainer-configuration-guide.md)
for the layer-list semantics.

Use `--help` on each command for the full flag list and runtime-flag
inclusion.

## Bare `agentmux` dispatch

Without a subcommand, the binary dispatches based on whether standard
input is a TTY:

- interactive TTY: starts `agentmux tui`
- non-interactive context: prints help and exits non-zero

## Shared runtime flags

All primary commands accept the runtime-root override flags. These
resolve configuration, state, and inscription roots in priority order:
explicit flag > environment variable > XDG default > home default.

- `--configuration-directory PATH`
- `--state-directory PATH`
- `--inscriptions-directory PATH` (alias: `--logs-directory PATH`)

For the resolution tiers and the closed-list semantics of the
configuration layer list, see the
[maintainer configuration guide](maintainer-configuration-guide.md) and
the [operations guide](operations.md).

## Operational references

- Login-time startup, service examples, shared runtime flags, and
  runtime artifact locations: [operations.md](operations.md).
- TUI session identity resolution, browsing-bundle selection, and
  keybindings: [tui.md](tui.md).
- See [maintainer-configuration-guide.md](maintainer-configuration-guide.md)
  for file-by-file configuration root contents, layering, and starter
  hydration.
- Authorization policy scopes, the per-control ladder, and the home
  namespace concept: [authorization.md](authorization.md).
- Multi-worktree topology and association resolution:
  [multi-worktree-workflow.md](multi-worktree-workflow.md).
