# Operations Guide

This guide covers runtime flags, service startup, and runtime artifact
locations for operators.

## Shared Runtime Flags

All primary commands support these runtime root overrides:

- `--configuration-directory PATH`
- `--state-directory PATH`
- `--inscriptions-directory PATH` (alias: `--logs-directory PATH`)
- `--repository-root PATH`

## Configuration Layout

Configuration lives outside any project checkout, in a directory you manage
yourself. Everything under it is specific to one maintainer: policies encode
your lane topology, `users.toml` names you, and `coders.toml` records the coder
CLIs you have installed. A configuration root holds:

```
coders.toml      # coder definitions
policies.toml    # authorization policy presets
relay.toml       # relay settings and peers
users.toml       # operator sessions and their policies  (optional)
ui.toml          # UI-surface defaults                    (optional)
bundles/*.toml   # one file per bundle
```

### Layering

`--configuration-directory` is repeatable. Each occurrence appends one layer,
and the layers are searched **in the order given — the first occurrence wins**.
This is a search path, like `PATH` or `-I` include directories: a lookup finds
a file and stops. Nothing is merged, so a layer that supplies `relay.toml`
replaces the whole file rather than contributing keys to it.

The gesture for adding an override is therefore to **prepend** it:

```bash
agentmux host relay \
  --configuration-directory ~/config/agentmux-rnd \
  --configuration-directory ~/config/agentmux
```

Here an R&D layer sits ahead of the shared base. If `~/config/agentmux-rnd`
holds only `relay.toml` and `bundles/scratch.toml`, then `relay.toml` and the
`scratch` bundle resolve from it while `coders.toml`, `policies.toml`, and every
other bundle resolve from the base. Resolution is per file, not per layer.

Bundle directories are the one exception to whole-file replacement, and it is
still not merging: the `bundles/` directories **union by identifier**, so a
layer redefining one bundle need not restate the others. A bundle present in
two layers resolves from the earlier one.

The environment form is `:`-separated, in the same order:

```bash
export AGENTMUX_CONFIGURATION_DIRECTORY=~/config/agentmux-rnd:~/config/agentmux
```

Paths containing `:` cannot be expressed this way; use the repeatable flag
instead. Empty elements are rejected rather than treated as the working
directory, so a leading, trailing, or doubled `:` is an error.

A supplied layer list is **closed**: no root outside the list is consulted, so
naming a layer that does not exist is an error rather than a silent fall-through
to the layers below it. This does not change what absence means — `users.toml`
and `ui.toml` are still optional, and being absent from every layer is still
fine for them.

### Inspecting which layer won

A shadowed file is present, valid, and entirely inert, which makes an edit that
does nothing the characteristic failure of a layered setup. `agentmux check
configuration` reports the physical file supplying each artifact it resolves:

```console
$ agentmux check configuration
source coders.toml: /home/you/config/agentmux/coders.toml
source policies.toml: /home/you/config/agentmux/policies.toml
source relay.toml: /home/you/config/agentmux-rnd/relay.toml
source bundles/scratch.toml: /home/you/config/agentmux-rnd/bundles/scratch.toml
source bundles/work.toml: /home/you/config/agentmux/bundles/work.toml
ok: scratch
ok: work
checked 2 bundle configuration(s): all valid
```

If the file you edited is not on that list, some earlier layer is shadowing it.
Only artifacts a layer actually supplies are reported, so an absent optional
file simply has no line. The report is written before validation runs, so it is
complete even on a run that fails, and it goes to standard output while failures
go to standard error. Pass `-q`/`--quiet` to suppress the success output — the
source report, the per-bundle lines, and the summary — leaving the exit code and
any failure report.

### Migrating from `overlay/`

The `overlay/` subdirectory convention is gone. A layer is now an ordinary
configuration root, named on the command line.

**This failure is silent.** An `overlay/` directory simply stops being
consulted; the base configuration below it is valid and loads cleanly, so
nothing errors — the deployment just runs on the files the overlay used to
override. Move the directory to a sibling and name it as a layer ahead of the
base:

```bash
mv ~/config/agentmux/overlay ~/config/agentmux-local
agentmux check configuration \
  --configuration-directory ~/config/agentmux-local \
  --configuration-directory ~/config/agentmux
```

Use the source report above to confirm the moved files are the ones in effect.

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
WorkingDirectory=%h/.local/share/agentmux
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

Authorization for relay operations (`list`, `look`, `send`, `raww`, `choose`,
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
