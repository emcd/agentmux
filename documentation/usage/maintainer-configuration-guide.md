# Maintainer Configuration Guide

This guide describes a single maintainer's configuration root: what it holds,
what each file is for, and how layering, absolute paths, and the related
state root interact. It is the operator-facing entry point for someone
setting up or extending an Agentmux deployment; the
[operations guide](operations.md) covers runtime flags and runtime
artifacts, and `src/configuration/README.md` covers the implementation.
This guide is the bridge.

## Audience and scope

You are reading this because you maintain an Agentmux deployment and need
to know what to put under your configuration root. The guide assumes the
configuration lives outside any project checkout, in a directory you
manage yourself — a maintainer's paths, lane topology, and bundle
definitions belong with the operator, not in the tool repo. The tool
repo carries the tool's own configuration surface (starter templates
under `data/configuration/`) but no deployment configuration; every file
under your root is yours, and would not belong to a second maintainer.

If you are reading code instead, start at
[`src/configuration/README.md`](../../src/configuration/README.md) and
[`src/runtime/README.md`](../../src/runtime/README.md). They describe the
same layout from the other side of the wall.

## What a configuration root contains

A configuration root is an ordinary directory. It holds six named TOML
files at the root, plus a `bundles/` subdirectory whose contents are
operator-defined bundle definitions. The schema is fixed; the contents
are yours.

```
coders.toml      # coder definitions                  (required)
policies.toml    # authorization policy presets       (required)
relay.toml       # relay-wide settings and peers      (optional)
users.toml       # operator identities and policies   (optional)
ui.toml          # UI-surface defaults                (optional)
mcp.toml         # MCP server association             (optional)
bundles/         # one file per bundle
  *.toml
```

The file names are fixed. There is no environment variable or flag that
changes them — they are the loader's lookup keys, declared once in
[`src/configuration/mod.rs`](../../src/configuration/mod.rs).

The starter templates under `data/configuration/` are the canonical
shape for the scaffolded files (`coders.toml`, `policies.toml`,
`relay.toml`, `users.toml`, `ui.toml`, and the example
`bundles/example.toml`). They are written out to a defaulted layer on
first use (`src/runtime/starter.rs:104-126`); what you put in the file
from then on is your configuration, not theirs. `mcp.toml` is the one
exception — it is intentionally not scaffolded, since an MCP server
creates its association only when it needs to bind to a specific bundle
and session.

### `coders.toml` — coder definitions

A coder is the agent CLI a bundle session launches. Coders are reusable
across bundles: one `claude` coder entry is referenced by every bundle
that wants Claude. Coder entries are pure definitions — no policy, no
relay peer — and are not live-reloaded by themselves. The bundle
watcher fingerprints effective bundle files
(`src/relay/watcher.rs:181-263`); a `coders.toml` edit by itself
leaves loaded bundles unchanged. Coder definitions take effect on relay
startup or when a bundle is freshly loaded or reloaded after its own
file changes.

The starter template ships five entries: `claude`, `codex`, `gemini`,
`opencode` (Tmux), and `opencode-acp` (ACP stdio). Each `[[coders]]`
declares exactly one of `[coders.tmux]`, `[coders.acp]`, or
`[coders.pty]`; the validator rejects both or neither
(`src/configuration/loaders.rs:491-517`). The `[coders.pty]` table is
gated behind the `pty` Cargo feature and ships commented-out in the
starter template.

What lives in a coder entry:

- `id` — the bundle's `coder = "<id>"` reference.
- `[coders.tmux]` / `[coders.acp]` / `[coders.pty]` — the transport.
  The starter Tmux form carries `initial-command`, `resume-command`,
  `prompt-regex`, `prompt-inspect-lines`, and `prompt-idle-column`;
  `prime-timeout-ms` and `wedge-detection` are optional bounded-prime
  and wedge-classifier switches
  ([`src/configuration/raw.rs`](../../src/configuration/raw.rs)).
- `[[coders.environment]]` — the base merge layer for a member's spawn
  environment, applied before the bundle and session layers
  ([`src/configuration/loaders.rs:543-559`](../../src/configuration/loaders.rs)
  applies the per-variable, most-specific-wins merge).

### `policies.toml` — authorization policy presets

Authorization for relay operations (`list`, `look`, `send`, `raww`,
`choose`, `updown`) resolves against named policy presets. A preset
maps each control to one of `none`, `self`, `home`, or `all`. The
configured scope has to meet the operation's minimum.

The starter template ships two presets: `default` (conservative
same-bundle policy; `raww`, `choose`, `updown` left at their built-in
`none` by being omitted) and `operator` (cross-namespace inspection,
messaging, choice decisions, and lifecycle — every control explicitly
set). The `default` preset omits three controls rather than shadowing
them — an omitted control resolves to its built-in default, not an
override, so the safer omission is intentional. The starter file
comments the omitted `raww`/`choose`/`updown` lines so the omission is
self-documenting; `operator` is self-documenting by setting every
control explicitly.

What lives in the file:

- `format-version` — schema version, currently `1`.
- `default` — preset id to apply when a session omits `policy`.
- `[[policies]]` — one entry per preset; each carries `id`,
  `description`, and `[policies.controls]`.

`updown` is deny by default (`src/runtime/README.md:182-187`). A session
that does not grant `updown = "home"` cannot bring bundles up or down;
`agentmux up` and `agentmux down` will surface a typed
`authorization_forbidden` error.

### `relay.toml` — relay-wide settings

This file is optional. A missing `relay.toml` resolves to all defaults,
so the starter template is a fully commented all-defaults file
(`data/configuration/relay.toml`). Keys live at the file root — do not
nest them under a `[relay]` table.

What lives in the file when present:

- `watch-bundles` — whether the relay watches the bundles configuration
  directory and reconciles running bundles when files change. Default
  `true`. Override precedence: `--no-watch` > `AGENTMUX_RELAY_WATCH_BUNDLES`
  env > this key.
- `require-session-credentials` — whether the relay enforces recognized
  session credentials on Hello. Default `false`. Precedence:
  `--require-credentials` > `AGENTMUX_RELAY_REQUIRE_SESSION_CREDENTIALS`
  env > this key.
- `[choices].pending-max` — bounded depth of the per-bundle choices
  queue. Default `256`, range `1..=4096`. No CLI or env override.
- `[[peers]]` — outbound peer relay endpoints, each with `alias`,
  `address` (the peer's absolute Unix socket path), and `connect-as`.
  Raw peer PSKs are never stored here — they live owner-only under
  `<state-root>/peers/<alias>.psk` while the principal store keeps only
  hashes.

### `users.toml` — operator identities and policies

`users.toml` is the identity file: it names the people and embedded
agents who connect as `@GLOBAL` relay-wide principals. Each
`[[sessions]]` entry declares one principal with a canonical
`session@GLOBAL` id, an optional human-facing `name`, a `policy`
preset reference, and exactly one session-type subtable (`[sessions.ui]`
for TUI operators, `[sessions.pubsub]` for embedded agents).

This file is distinct from `ui.toml`: identity and policy here, surface
preferences there. A missing `users.toml` is fine; an MCP server that
needs an association sets one via `--bundle` / `--session-name`
instead.

The starter template ships one entry: `user@GLOBAL` mapped to the
`operator` policy with a `[sessions.ui]` subtable.

### `ui.toml` — UI-surface defaults

Read-only operational defaults for the interactive surfaces (TUI,
CLI), kept separate from `users.toml`. The starter template is a fully
commented all-defaults file with one optional key:

- `default-bundle` — the bundle the TUI browses by default, and the
  bundle one-shot `agentmux send` uses when `--bundle` is omitted.
  When unset, the TUI falls back to the first available bundle while
  `agentmux send` requires an explicit `--bundle`.

A missing `ui.toml` resolves to no configured defaults. This file is
read-only: Agentmux never writes to it.

### `mcp.toml` — MCP server association

Not scaffolded. The operator creates this file when an MCP server
needs to bind to a specific bundle and session: it carries
`bundle_name` and `session_name`, which the MCP server reads at
startup (`src/runtime/association.rs:45-71`). Agentmux never writes
`mcp.toml`; missing files simply resolve through the remaining
precedence tiers. A missing `mcp.toml` does not prevent association;
MCP resolves identity through a four-tier precedence
(`src/runtime/association.rs:164-203`):

- **Bundle**: `--bundle` > injected `AGENTMUX_BUNDLE` > `mcp.toml` >
  `--default-bundle`. A supplied identity which does not resolve to a
  known member is a fault, not a fallback to a lower tier
  (`src/runtime/association.rs:142-145`).
- **Session**: `--session-name` > injected `AGENTMUX_SESSION` >
  `mcp.toml`. A working-directory match against declared member
  directories applies after the bundle resolves, as a separate inference
  step (`src/runtime/association.rs:240-266`,
  [`src/runtime/README.md:14-20`](../../src/runtime/README.md)).

### `bundles/<id>.toml` — one file per bundle

A bundle is a named set of sessions the relay starts together. Each
bundle lives in its own file under `bundles/`, named by the bundle
identifier. The starter template writes a commented-out
`bundles/example.toml` only when no bundles are present
(`src/runtime/starter.rs:115-126`); subsequent edits are yours.

What lives in a bundle file:

- `format-version` — schema version, currently `1`.
- `autostart` — whether the relay starts the bundle on its own
  startup. Default `false`.
- `groups` — group identifiers for collective lifecycle operations
  (`agentmux up --group` / `agentmux down --group`).
- `[[sessions]]` — one entry per session. Each carries:
  - `id` — canonical routing identity.
  - `name` — optional human-facing label.
  - `directory` — the absolute path the session runs from. See
    [Bundle member `directory` semantics](#bundle-member-directory-semantics).
  - `policy` — optional policy preset reference; defaults to the
    `policies.toml` `default` preset.
  - `coder` — optional coder id from `coders.toml`.
  - `coder-session-id` — optional persistent agent session handle;
    ACP selects `session/load` vs `session/new` from it.
  - `[[sessions.environment]]` — the most-specific merge layer,
    overriding bundle and coder entries of the same name.
  - `[sessions.ui]` or `[sessions.pubsub]` — only on a session that
    is not coder-backed.

A bundle definition only one layer defines is enumerated; a definition
shadowing one of the same identifier in a later layer is enumerated
once, at the path that supplied it
([`src/configuration/paths.rs:90-111`](../../src/configuration/paths.rs)).
A layer can shadow a bundle but cannot remove one — there is no
tombstone. Replacing a set wholesale means shadowing each member
individually.

## Where configuration is *not* stored

The tool repository holds no deployment configuration. Nothing under
`src/`, `data/configuration/`, or any other checked-in path is your
deployment; every file under a configuration root is maintainer-
specific.

This is the test, not a heuristic: would a second maintainer want the
file's contents? For every file under your root the answer is no.
`policies.toml` encodes your lane topology, `users.toml` names you,
`coders.toml` records the coder CLIs you have installed, and bundle
files carry your absolute worktree paths.

The starter templates under `data/configuration/` look like your
configuration but are not: they are scaffolded into your root on first
use (`src/runtime/starter.rs:104-126`) and then they are yours to edit
or replace. Reinstalling the tool does not overwrite them; the
hydration check skips a file that already exists
(`src/runtime/starter.rs:135-141`).

## Configuration root vs state root

The two roots resolve by different rules and answer different questions.
Operators conflate them because both are directories on disk and both
are passed to the same commands; the difference is what lives in each.

The **configuration** root holds your settings. Its resolution tiers
are:

1. `--configuration-directory PATH` (repeatable)
2. `AGENTMUX_CONFIGURATION_DIRECTORY` (colon-separated list)
3. `$XDG_CONFIG_HOME/agentmux`, else `~/.config/agentmux`

The **state** root holds where a relay lives — the relay socket, the
locks, the ready sentinel, the principal store, peer credentials, and
the per-bundle runtime directories. Its resolution tiers are:

1. `--state-directory PATH`
2. `AGENTMUX_STATE_DIRECTORY`
3. `$XDG_STATE_HOME/agentmux`
4. `~/.local/state/agentmux`

The **state** root is normalized to an absolute path after resolution
([`src/runtime/paths.rs:326-339`](../../src/runtime/paths.rs)); the
**configuration** layer list is not — explicit and environment tiers
pass through their declared `PathBuf`s, so a relative configuration
layer resolves against the process working directory at lookup time
([`src/runtime/paths.rs:259-284`](../../src/runtime/paths.rs)).
A blank `AGENTMUX_*` environment value is treated as absent and
resolution falls to the next tier
([`src/runtime/paths.rs:357-365`](../../src/runtime/paths.rs));
empty CLI values and empty elements in a non-blank layer list are
rejected
([`src/configuration/roots.rs:101-129`](../../src/configuration/roots.rs)).

One state root is one relay
([`src/runtime/README.md:77-83`](../../src/runtime/README.md)). Isolating
two deployments means naming two state roots and nothing else does it;
no identifier is inferred from your configuration, your build, or
where you launched from. Selecting a configuration root does not
identify or isolate a relay — `host relay` writes missing starter files
to a defaulted list (`src/runtime/starter.rs:92-128`), so the
configuration root is not read-only to the relay. State root is what
isolates a relay; two relays sharing one configuration root is fine.

Running two relays against one configuration root is fine. Running two
relays against one state root is a conflict. The relay stamps its own
state root into every member it spawns as `AGENTMUX_STATE_DIRECTORY`,
overwriting any operator-declared value. The single overwrite lives
on the spawn path:
[`src/relay/lifecycle.rs:101-108`](../../src/relay/lifecycle.rs)
(`stamp_hosted_bundle`) calls
[`src/configuration/loaders.rs:577-590`](../../src/configuration/loaders.rs)
(`inject_spawn_state_directory`). Routing the stamp through one
chokepoint means a member spawned by any of the three paths — first
startup, `up`/reconcile, or a lazy Pty delivery load — is pointed at
the same relay, and an operator-declared value being overwritten
breaks the rendezvous rather than expressing a preference.

## A worked minimal deployment

One bundle, one coder, one policy, one user. Files live at
`~/.config/agentmux/`. Commands run against the defaulted single layer.

### `coders.toml`

```toml
# One coder: codex, via Tmux.
format-version = 1

[[coders]]
id = 'codex'

[coders.tmux]
initial-command = 'codex'
resume-command = 'codex resume {coder-session-id}'
prompt-regex = '(?ms)^›.*\n\s*\n.*$'
prompt-inspect-lines = 3
prompt-idle-column = 2
```

### `policies.toml`

```toml
format-version = 1
default = 'operator'

[[policies]]
id = 'operator'
description = 'Operator policy with cross-namespace reach.'

[policies.controls]
choose = 'home'
find = 'self'
list = 'all'
look = 'all'
raww = 'all'
send = 'all'
updown = 'home'

[policies.controls.new]
peer = 'all'

[policies.controls.change]
psk = 'all'
```

### `users.toml`

```toml
default-session = 'user@GLOBAL'

[[sessions]]
id = 'user@GLOBAL'
name = 'Operator'
policy = 'operator'

[sessions.ui]
```

### `ui.toml`

```toml
# All defaults. Uncomment to set a default bundle.
# default-bundle = 'work'
```

### `relay.toml`

Either present as the all-defaults starter template, or absent. Both
behave identically.

### `bundles/work.toml`

```toml
format-version = 1
autostart = true

[[sessions]]
id = 'agent-a'
name = 'Agent A'
directory = '/home/you/src/work/agent-a'
policy = 'operator'
coder = 'codex'
```

### Start it

```bash
agentmux host relay
```

The defaulted configuration root is `~/.config/agentmux`; the defaulted
state root is `~/.local/state/agentmux`. The relay reads `coders.toml`
and `policies.toml` from the configuration root, registers the
`work` bundle, and starts `agent-a` because `autostart = true`.

## A worked layering example

A shared base plus an R&D variant. The variant sits *ahead* of the base
because the first layer wins.

Layout:

```
~/config/agentmux/         # shared base
  coders.toml
  policies.toml
  users.toml
  bundles/work.toml

~/config/agentmux-rnd/     # R&D variant
  relay.toml               # one-off setting, overrides base
  bundles/scratch.toml     # one-off bundle, no counterpart in base
```

### Base layer

`~/config/agentmux/coders.toml`:

```toml
format-version = 1

[[coders]]
id = 'codex'

[coders.tmux]
initial-command = 'codex'
resume-command = 'codex resume {coder-session-id}'
prompt-regex = '(?ms)^›.*\n\s*\n.*$'
prompt-inspect-lines = 3
prompt-idle-column = 2
```

`~/config/agentmux/policies.toml`:

```toml
format-version = 1
default = 'default'

[[policies]]
id = 'default'
description = 'Conservative same-bundle policy.'

[policies.controls]
find = 'self'
list = 'home'
look = 'home'
send = 'home'

[[policies]]
id = 'operator'
description = 'Operator policy with cross-namespace reach.'

[policies.controls]
choose = 'home'
find = 'self'
list = 'all'
look = 'all'
raww = 'all'
send = 'all'
updown = 'home'
```

`~/config/agentmux/users.toml`:

```toml
default-session = 'user@GLOBAL'

[[sessions]]
id = 'user@GLOBAL'
name = 'Operator'
policy = 'operator'

[sessions.ui]
```

`~/config/agentmux/bundles/work.toml`:

```toml
format-version = 1
autostart = true

[[sessions]]
id = 'agent-a'
name = 'Agent A'
directory = '/home/you/src/work/agent-a'
policy = 'default'
coder = 'codex'
```

### R&D layer

`~/config/agentmux-rnd/relay.toml`:

```toml
# Disable live bundle reconciliation in the R&D relay; bundle
# edits take effect on the next restart, not mid-run.
watch-bundles = false
```

`~/config/agentmux-rnd/bundles/scratch.toml`:

```toml
format-version = 1

[[sessions]]
id = 'agent-scratch'
name = 'Scratch Agent'
directory = '/home/you/rnd/scratch'
policy = 'default'
coder = 'codex'
```

### Invocation

```bash
agentmux host relay \
  --configuration-directory ~/config/agentmux-rnd \
  --configuration-directory ~/config/agentmux
```

Resolution per file:

| File                       | Effective lookup                          |
|----------------------------|-------------------------------------------|
| `coders.toml`              | `~/config/agentmux/coders.toml` (base)    |
| `policies.toml`            | `~/config/agentmux/policies.toml` (base)  |
| `users.toml`               | `~/config/agentmux/users.toml` (base)     |
| `relay.toml`               | `~/config/agentmux-rnd/relay.toml` (R&D)  |
| `bundles/work.toml`        | `~/config/agentmux/bundles/work.toml`     |
| `bundles/scratch.toml`     | `~/config/agentmux-rnd/bundles/scratch.toml` |

`coders.toml`, `policies.toml`, and `users.toml` are not in the R&D
layer, so the lookup falls through to the base. `relay.toml` exists
only in the R&D layer and resolves there. `bundles/` directories
union by identifier (`src/configuration/paths.rs:90-111`); `work` and
`scratch` have distinct identifiers, so both are enumerated.

The relay reports the source of every file it actually loads, so an
edit that lands in the wrong layer is visible immediately. See
[Inspecting which layer won](#inspecting-which-layer-won).

### The environment form

The colon-separated form carries the same order:

```bash
export AGENTMUX_CONFIGURATION_DIRECTORY=~/config/agentmux-rnd:~/config/agentmux
agentmux host relay
```

A path containing `:` cannot be expressed here; the repeatable flag is
the escape hatch. Empty elements (leading, trailing, or doubled `:`)
are rejected rather than read as the working directory — reading a
layer from wherever a process was started is a privilege question, not
a convenience (`src/configuration/roots.rs:101-129`).

A supplied list is closed: no root outside it is consulted for any
file, so a typo is an error rather than a silent demotion to a
different deployment. Optional files (`users.toml`, `ui.toml`,
`mcp.toml`) stay optional — closedness governs *which roots are
searched*, not what absence means.

## Bundle member `directory` semantics

`directory` on a `[[sessions]]` entry is **absolute today**. The
loader does not rebase relative paths against any layer: relative
values resolve against the process working directory, which is rarely
what you want. A session running from `~/src/work/agent-a` should
declare it as `/home/you/src/work/agent-a`, not `~/src/work/agent-a`
or `src/work/agent-a`.

This is on the path-base question in `todos/runtime/20`: a per-member
or per-bundle rebase against the configuration layer that supplied
the file would let you write `bundles/work.toml` once and share it
across hosts with different home directories. Today it does not work
that way. Track that todo for the future resolution.

## Migrating from `overlay/`

The `overlay/` subdirectory convention is gone. A layer is now an
ordinary configuration root, named on the command line.

**This failure is silent.** An `overlay/` directory simply stops being
consulted; the base configuration below it is valid and loads cleanly,
so nothing errors — the deployment just runs on the files the overlay
used to override. The loader reports only files it actually loads, so
the disappearance is invisible without inspecting the filesystem
directly.

Move the directory to a sibling and name it as a layer ahead of the
base:

```bash
mv ~/config/agentmux/overlay ~/config/agentmux-local
agentmux check configuration \
  --configuration-directory ~/config/agentmux-local \
  --configuration-directory ~/config/agentmux
```

`agentmux check configuration` reports the physical file supplying
each artifact it resolves; see
[Inspecting which layer won](#inspecting-which-layer-won). Use the
report to confirm the moved files are the ones in effect.

## Inspecting which layer won

A shadowed file is present, valid, and entirely inert, which makes an
edit that does nothing the characteristic failure of a layered setup.
`agentmux check configuration` reports the physical file supplying
each artifact it resolves:

```console
$ agentmux check configuration
source coders.toml: /home/you/config/agentmux/coders.toml
source policies.toml: /home/you/config/agentmux/policies.toml
source relay.toml: /home/you/config/agentmux-rnd/relay.toml
source users.toml: /home/you/config/agentmux/users.toml
source bundles/scratch.toml: /home/you/config/agentmux-rnd/bundles/scratch.toml
source bundles/work.toml: /home/you/config/agentmux/bundles/work.toml
ok: scratch
ok: work
checked 2 bundle configuration(s): all valid
```

If the file you edited is not on that list, some earlier layer is
shadowing it. Only artifacts a layer actually supplies are reported,
so an absent optional file simply has no line. The report is written
before validation runs, so it is complete even on a run that fails;
success output goes to stdout, failures to stderr. Pass
`-q`/`--quiet` to suppress the success output — the source report,
the per-bundle lines, and the summary — leaving the exit code and any
failure report.

## Quick reference

### Root resolution tiers

| Tier | Configuration                          | State                       |
|------|----------------------------------------|-----------------------------|
| 1    | `--configuration-directory PATH` (×N)  | `--state-directory PATH`    |
| 2    | `AGENTMUX_CONFIGURATION_DIRECTORY`     | `AGENTMUX_STATE_DIRECTORY`  |
| 3    | `$XDG_CONFIG_HOME/agentmux`            | `$XDG_STATE_HOME/agentmux`  |
| 4    | `~/.config/agentmux`                   | `~/.local/state/agentmux`   |

### Conventions at a glance

| Concern            | Answer                                            |
|--------------------|---------------------------------------------------|
| Layer separator    | `:` (matches `PATH`/`LD_LIBRARY_PATH` conventions)|
| Layer ordering     | first layer wins (search-path semantics, not merge)|
| Bundle directories | union by identifier (no merge, no tombstone)      |
| Schema version key | `format-version = N` (coders, bundles, policies)  |
| `directory` field  | absolute path today; relative resolves against cwd |
| Starter hydration  | applies only to a defaulted list (single layer)   |
| Bundles directory  | `<config-root>/bundles/<id>.toml`                 |
| Inscriptions       | `<state-root>/inscriptions` by default            |
| Relay socket       | `<state-root>/relay.sock`                         |

For runtime flags and runtime artifacts (sockets, locks, ready
sentinel, systemd unit), see the
[operations guide](operations.md). For the implementation details of
the layer list and the lookup, see
[`src/configuration/README.md`](../../src/configuration/README.md) and
[`src/runtime/README.md`](../../src/runtime/README.md).
