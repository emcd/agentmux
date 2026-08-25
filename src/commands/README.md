# Commands Module

This directory owns the unified CLI surface for `agentmux`.

## Responsibilities

- Parse command-line arguments into typed command structs.
- Validate command inputs before runtime/relay calls.
- Resolve runtime root overrides for every command family.
- Render human and machine output for command responses.

## File Map

- `mod.rs`
  - top-level command router and shared command argument/result structs.
- `host/mod.rs`
  - `agentmux host` module hub.
- `host/arguments.rs`
  - `agentmux host relay` and `agentmux host mcp` argument parsing.
- `host/help.rs`
  - `agentmux host` help text rendering.
- `host/mcp.rs`
  - `agentmux host mcp` association resolution, inscription setup, and MCP
    stdio service startup.
- `host/relay.rs`
  - relay listener/process loop, no-autostart mode, startup orchestration,
    connection-worker orchestration, and bundle file watcher lifecycle (spawned
    after startup unless `--no-watch`, dropped before cleanup).
- `host/router.rs`
  - `agentmux host` mode dispatch.
- `host/summary.rs`
  - relay startup summary construction, JSON payload rendering, and per-bundle
    outcome helpers.
- `bundle.rs`
  - shared `up`/`down` transition execution helpers, including the rendering of
    per-session failure detail.
- `up.rs`
  - `agentmux up` selector parsing and execution.
- `down.rs`
  - `agentmux down` selector parsing and execution.
- `list.rs`
  - `agentmux list principals`.
- `look.rs`
  - `agentmux look`.
- `raww.rs`
  - `agentmux raww` direct-write request surface.
- `new.rs`
  - `agentmux new peer <principal_id>` credential registration; relays a
    `NewPeer` request and renders the returned PSK + config snippet (or reports
    the `--output` path the PSK was written to).
- `change.rs`
  - `agentmux change psk <principal_id>` credential rotation; relays a
    `ChangePsk` request and renders the new PSK.
- `drop.rs`
  - `agentmux drop peer <principal_id>` principal deletion; relays a `DropPeer`
    request and reports the deleted principal plus, for session principals only,
    the relay-owned credential file left behind for the operator to remove.
- `check.rs`
  - `agentmux check configuration [<bundle-id>]` read-only configuration
    pre-flight; validates one or all bundles through the relay's startup loading
    path (`relay::preflight_bundle_configuration`) and exits non-zero on the
    first invalid bundle with file path + field-level detail. Never scaffolds or
    mutates configuration.
- `send.rs`
  - `agentmux send`, including stdin/message precedence and timeout fields.
- `tui.rs`
  - `agentmux tui` launch path, session/default precedence wiring, and relay
    auto-spawn fallback using resolved runtime roots.
- `shared.rs`
  - reusable parsing/output helpers shared across command handlers.

## Operational Notes

- Bare `agentmux` dispatches to TUI only in interactive TTY mode.
- The top-level router handles two global flags before subcommand dispatch:
  `-h`/`--help` prints the command usage and `-V`/`--version` prints
  `agentmux <crate-version>` (sourced from `CARGO_PKG_VERSION`); both exit
  successfully.
- `host relay --no-autostart` is process-only mode and must not report
  autostart failures for bundles.
- A bundle that comes up with some sessions failing reports `outcome=degraded`
  on both bring-up surfaces (`host relay`'s startup summary and `up`'s
  transition summary), carrying the failed session ids and causes in `reason`
  and `details.failed_sessions`, and counted in `degraded_bundle_count`. It is a
  hosted outcome: it keeps `hosted_any`/`changed_any` true, stays out of
  `failed_bundle_count`, and does not by itself make the host exit nonzero.
  `outcome=failed` is reserved for a bundle with no ready session. The spelling
  is the one `list` already reports as `startup_health`, under the same
  predicate.
- `host relay` watches the bundles configuration directory by default and
  reconciles loaded bundles on change (add/remove/modify); `--no-watch` disables
  this for the process lifetime.
- Worker-pool overload and pre-hello idle handling are implemented in
  `host/relay.rs` and covered by integration tests under `tests/integration/cli/`
  and `tests/integration/relay_delivery_runtime.rs`.
