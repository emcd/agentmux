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

## Runtime Artifacts

Per-bundle state directory:

- `<state-root>/bundles/<bundle-name>/`

Important files:

- `relay.sock`: relay Unix socket
- `tmux.sock`: bundle tmux socket
- `relay.lock`: relay host lock
- `relay.spawn.lock`: relay spawn lock

Inscriptions:

- relay log:
  `<inscriptions-root>/bundles/<bundle-name>/relay.log`
- MCP log:
  `<inscriptions-root>/bundles/<bundle-name>/sessions/<session-name>/mcp.log`
