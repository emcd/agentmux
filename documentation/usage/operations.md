# Operations Guide

This guide covers runtime flags, service startup, and runtime artifact
locations for operators.

## Shared Runtime Flags

All primary commands support these runtime root overrides:

- `--config-directory PATH`
- `--state-directory PATH`
- `--inscriptions-directory PATH` (alias: `--logs-directory PATH`)
- `--repository-root PATH`

## Auto Start On Login

### Systemd (--user)

When `agentmux` is installed via `cargo install`, create:

`~/.config/systemd/user/agentmux-relay.service`

```ini
[Unit]
Description=agentmux relay host
After=default.target

[Service]
Type=simple
ExecStart=/usr/bin/env agentmux host relay
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
```

Enable and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now agentmux-relay.service
systemctl --user status agentmux-relay.service
```

Follow logs:

```bash
journalctl --user -u agentmux-relay.service -f
```

If coder CLIs are installed in non-default locations (for example via
Mise/Asdf/NVM or a custom npm prefix), add explicit environment in the unit:

```ini
[Service]
Environment=PATH=/path/to/node/bin:/path/to/cargo/bin:/path/to/npm-prefix/bin:/usr/local/bin:/usr/bin:/bin
Environment=CODEX_HOME=/path/to/codex/home
Environment=CLAUDE_CONFIG_DIR=/path/to/claude/config
```

After environment edits:

```bash
systemctl --user daemon-reload
systemctl --user restart agentmux-relay.service
```

## Authorization Policies

Authorization for relay operations (`list`, `look`, `send`, `raww`, `grant`,
`updown`) is configured in `<config-root>/policies.toml`. Each policy preset
sets a scope (`none`, `self`, `home`, `all`) per control; the
configured scope must meet the operation's minimum.

The `updown` control gates the `agentmux up` and `agentmux down` commands
(and the corresponding `RelayRequest::Up` and `RelayRequest::Down` requests
to the relay). It is **deny by default** — a session whose policy does not
grant `updown = "home"` cannot bring bundles up or down. Configured
operators (the starter `operator` policy in the scaffolded
`policies.toml`) carry this grant; the conservative `default` policy does
not.

A bundle lifecycle request without an authorized principal receives a
typed `authorization_forbidden` error from the relay; the CLI surfaces it
as a `relay returned error: authorization_forbidden` message. Operators who
hit this should verify that `users.toml` maps their `session@GLOBAL`
identity to a policy with `updown = "home"`.

## Runtime Artifacts

Relay-level artifacts at the state root:

- `<state-root>/relay.sock`: single relay Unix socket; serves every
  configured bundle and routes connections by the `bundle_name` carried in
  each client's `hello` frame
- `<state-root>/relay.lock`: relay host lock
- `<state-root>/relay.spawn.lock`: relay spawn lock
- `<state-root>/relay.ready`: relay readiness sentinel (present only while
  the host is serving with signal handlers installed and the accept loop
  spawned)

Per-bundle state directory:

- `<state-root>/bundles/<bundle-name>/`

Important files:

- `tmux.sock`: bundle tmux socket

Inscriptions:

- relay log: `<inscriptions-root>/relay.log`
- MCP log:
  `<inscriptions-root>/bundles/<bundle-name>/sessions/<session-name>/mcp.log`
