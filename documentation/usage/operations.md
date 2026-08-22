# Operations Guide

This guide covers runtime flags, service startup, and runtime artifact
locations for operators.

## Shared Runtime Flags

All primary commands support these runtime root overrides:

- `--configuration-directory PATH`
- `--state-directory PATH`
- `--inscriptions-directory PATH` (alias: `--logs-directory PATH`)

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
typed `authorization_forbidden` validation error from the relay, which the
CLI surfaces as a validation-shaped result (not a `relay returned error: ...`
wrapper). The canonical operator-facing description of the authorization
model — per-control ladder, home-namespace rules, reachability, and the
`updown = "home"` requirement — lives in
[authorization.md](authorization.md); the
`users.toml` → `operator` policy mapping is the operator-side fix the
authorization doc describes in its `updown` is deny by default section.

## State Root

The state root is where a relay lives. It is resolved from four tiers, in
order, and the answer is identical in every build profile:

1. `--state-directory PATH`
2. `AGENTMUX_STATE_DIRECTORY`
3. `$XDG_STATE_HOME/agentmux`
4. `~/.local/state/agentmux`

The resolved root is normalized to an absolute path. An empty
`--state-directory` is an error; a blank `AGENTMUX_STATE_DIRECTORY` is treated
as absent, like every other environment tier.

**One state root is one relay.** Everything that distinguishes two deployments
— the relay socket, both locks, the ready sentinel, the principal store, peer
credentials — sits at the state root rather than under a bundle. Two relays
started against the same root contend for the same socket and spawn lock.

Isolating a deployment therefore means **naming its state root**, and nothing
else does it. No identifier is inferred from your configuration, your build, or
where you launched from. In particular, a source build and an installed build
launched with the same arguments share a relay; separate them by giving one of
them its own `--state-directory`.

The inscriptions root defaults to `<state-root>/inscriptions`, so it follows the
state root unless you name it separately with `--inscriptions-directory`.

### Propagation to spawned agents

A relay stamps its own state root into every member it spawns, as
`AGENTMUX_STATE_DIRECTORY`. The launched coder passes it to its
`agentmux host mcp` subprocess by ordinary environment inheritance, so the
child resolves the relay that started it rather than re-deriving a root of its
own.

That stamp is **authoritative**: it overwrites any `AGENTMUX_STATE_DIRECTORY`
declared at the coder, bundle, or session level. The variable names the relay a
member belongs to, so a declared value would not express a preference — it
would send the child to a relay that never spawned it, while the one that did
waits for a client that never arrives. Reaching another relay is expressed with
configured peers, not by re-pointing a child.

For the same reason, generated coder client configuration must not pass
`--state-directory`: a flag on a committed command line outranks the stamp.

A relay also stamps its configuration layer list, as
`AGENTMUX_CONFIGURATION_DIRECTORY`, so a member reads the declarations the relay
selected instead of resolving a root of its own. Generated client configuration
must not pass `--configuration-directory` either, for the same reason and with
an added one: a committed absolute path names one checkout, so every other
worktree reads that checkout's configuration.

This stamp is **not** authoritative. A value declared at the coder, bundle, or
session level is kept — unlike the state root, a divergent configuration root
does not break the rendezvous, because the socket and credentials resolve
beneath the state root regardless. It changes which declarations the member
reads.

Two consequences are worth knowing before you upgrade:

- A relative `--configuration-directory` or `AGENTMUX_CONFIGURATION_DIRECTORY`
  is made absolute against the relay's working directory when it starts, rather
  than re-resolving wherever a later lookup happens. If you relied on
  lookup-time re-resolution, name the intended path instead. Symlinked paths are
  left as you wrote them.
- A member whose configuration root does not exist now reports missing
  configuration rather than being given starter files. Previously such a member
  could be scaffolded into an empty deployment that appeared to work.

### Running two relays

Inter-relay work needs each relay to name its own state root:

```
agentmux host relay --state-directory ~/.local/state/agentmux-a
agentmux host relay --state-directory ~/.local/state/agentmux-b
```

A peer's `address` in `relay.toml` is the other relay's socket path beneath its
state root — `~/.local/state/agentmux-b/relay.sock` for the pair above. Because
you chose both roots, both addresses are known before either relay has run.

### Migrating off repository-local state

Earlier versions redirected debug builds to a repository-local state root,
derived from the Git checkout you launched inside. That is gone: the same
invocation now resolves the tiers above whatever the build profile.

If you relied on it, this is a **stop-the-relay** operation, but nothing moves
on disk. Stop the relay, then either keep using the old locations by naming
them, or start fresh on the default root and re-register credentials.

Name **both** roots if you keep them. The old state and inscriptions roots are
siblings, not nested:

```
agentmux host relay \
  --state-directory <checkout>/.auxiliary/state/agentmux \
  --inscriptions-directory <checkout>/.auxiliary/inscriptions/agentmux
```

Supplying only `--state-directory` preserves credentials and session state
while silently relocating new inscriptions to
`<checkout>/.auxiliary/state/agentmux/inscriptions`, splitting your log history
across two locations with nothing to indicate it happened.

Nothing changes for a deployment already resolving XDG.

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
