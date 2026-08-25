# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Inter-relay communication (headline 0.9.0 feature):** peers via
  `relay.toml` `[[peers]]` (`alias` / `address` / `connect-as`), PSK
  credential administration (`agentmux new peer` / `agentmux change psk`
  with `--output` / `--write-config` and `--scope`), cross-relay `send`
  via `session@bundle!peer` bang-path, relay-wide discovery
  (`list relays` / `discover`), and sender attribution
  (`on_behalf_of` on delivered envelopes and responses). The current peer
  credential is presented on every redial; unmatched ingress scopes are
  recorded rather than silently ignored.

- **Layered configuration roots:** repeatable `--configuration-directory`
  (first wins, per-file; `bundles/` unions by identifier) and
  `AGENTMUX_CONFIGURATION_DIRECTORY` (`:`-separated, same order).
  `overlay/` convention removed. The relay stamps
  `AGENTMUX_STATE_DIRECTORY` (authoritative) and
  `AGENTMUX_CONFIGURATION_DIRECTORY` at spawn, normalized to absolute
  paths; `agentmux check configuration` reports which file won each
  lookup.

- **State root unification:** single resolution tier
  (`--state-directory` / `AGENTMUX_STATE_DIRECTORY` / `XDG_STATE_HOME` /
  `~/.local/state/agentmux`) in every build profile. Repository-local
  debug state removed; one state root is one relay. Inscriptions follow
  the state root unless `--inscriptions-directory` overrides.

- **Pty transport stub:** `libghostty-vt`-backed Pty transport behind
  `Cargo.toml` feature `pty` (default build does not require it), wired
  throughout `src/pty/`. Graduation deferred past 0.9.0; known gaps
  remain. See `src/pty/README.md`.

- **Operations:** `agentmux up` / `agentmux down` now exit non-zero when
  any bundle transition failed.

### Changed

- **Delivery is now a relay-owned queue.** Per-coder prime / readiness /
  wedge timers are retired; a reachable but not-ready target waits
  indefinitely. What is bounded is the queue: per-target and relay-global
  admission quotas in `relay.toml` `[delivery]`, plus a separate
  `unreachable-dwell-ms` that resolves a continuously unreachable target
  as `not_submitted`. `submission-timeout-ms` is not yet enforced and is
  documented as such.

- **Startup health is grounded in readiness.** Bundle health
  (`healthy` / `degraded` / `down`) derives from whether each session's
  target is ready, not from recorded failures. One session failing does
  not fail the whole bundle bring-up.

- **Bundle watching is reliable on macOS:** the watcher now sweeps on
  both file notifications and a recurring interval, so a coalesced
  `[Create, Remove]` FSEvent no longer leaves a deleted bundle running.
  Graceful `SIGTERM`/`SIGINT` drains delivery workers and reaps ACP
  children before the process exits.

### Fixed

- Relay no longer loses the frame a shut-down socket still holds
  (macOS `EINVAL` on `SO_RCVTIMEO` arming).
- Reloading a bundle no longer inherits the prior generation's delivery
  worker — workers are stopped before tmux teardown.
- `relay.toml` peer and configuration-layer validation now reports the
  offending file and reason instead of `internal_unexpected_failure`.

## [0.8.0] - 2026-06-27

### Changed

- `raww` is now asynchronous, appears in TUI pending-delivery tracking,
  and its authorization default is tightened to `none` (explicit grant
  required).
- `bundle_name` removed from MCP inscription keys and responses; use
  canonical principal ids (`session@namespace`).
- Permission surface renamed from `grant` to `choose` / `choices`; the
  standalone `grant` MCP tool is retired (use the integrated choices
  surface).
- Policy scopes simplified to `home` and `all` (`policies.toml` is now
  authoritative for the full ladder; per-control caps removed).
- Policy validation and bundle startup failures are now surfaced in
  operator output.

### Fixed

- Reconnecting under an existing identity no longer exhausts the retry
  window when the previous owner is dead — the dead writer is evicted
  immediately and the error is `relay_timeout`.
- Relay readiness now requires `relay.ready` (signal handlers installed
  and accept loop running), not just socket existence.
- A bundle held by `down` (or `autostart = false`) stays held across
  file edits until an explicit `up`.

## [0.7.0] - 2026-06-10

### Added

- Relay peers with signed credentials: expired credentials rejected at
  Hello; live sessions disconnected on rotation/revocation; `new peer`
  / `change psk` via relay and MCP.
- Cross-bundle `send` / `look` / `raww` with uniform `home` / `all`
  authorization; `@GLOBAL` senders can address `session@bundle`
  targets; `authenticated_identity` on delivered envelopes.
- TUI: bundle switcher (F5), recipient persistence, unified
  Interaction-mode entry, `@GLOBAL` operator without a default bundle,
  per-session ready/hosted indicators, keybinding hints.
- MCP: `list` now takes a namespace selector and renames sessions to
  `principals`; `@GLOBAL` appears in listings; `updown` lifecycle tool;
  automatic relay reconnect; `look` windowing with `tail-N` + `offset`.
- Live bundle file watching: edits to `bundles/*.toml` reload without
  restarting the relay.

### Fixed

- Relay now exits cleanly on `SIGTERM` within a bounded grace period
  (watchdog guarantees exit even if the runtime is starved).

## [0.6.0] - 2026-05-26

### Changed

- One relay socket at `<state_root>/relay.sock` for all bundles; unknown
  bundles receive `validation_unknown_bundle`.
- Bundle lifecycle (`up` / `down`) now requires the `updown` policy
  control (`operator` preset grants `home`; `default` denies).
- `agentmux chat` renamed to `agentmux send` (wire protocol, CLI, MCP,
  and inscriptions).

### Added

- `users.toml` accepts bare ids (e.g. `user` → `user@GLOBAL`).

## [0.5.0] - 2026-05-19

### Changed

- All `send` delivery is now async-only (sync delivery mode removed;
  TUI and MCP updated).

### Fixed

- Reconnecting with the same identity now reclaims a dead owner
  immediately instead of waiting out the retry window (`relay_timeout`).

## [0.4.0] - 2026-05-16

### Added

- TUI Communication / Interaction two-mode surface.
- MCP `grant` tool for approving ACP permission requests.

### Changed

- More concurrent relay connections handled without pool exhaustion
  (larger worker pool; `AGENTMUX_RELAY_CONNECTION_WORKERS` still
  overrides).

## [0.3.0] - 2026-05-15

### Added

- Bracketed paste for tmux injection (preserves bracketed paste mode in
  the target pane).
- ACP workers auto-respawn after transport failures (1 s → 30 s backoff).
- Look overlay with full-screen permission panel.
- `raww` for raw input to coder sessions (CLI, TUI, MCP).
- Separate `opencode` (Tmux) and `opencode-acp` coder definitions.

### Changed

- Lower TUI CPU when idle.

## [0.2.0] - 2026-04-21

### Added

- Initial `agentmux` relay, TUI workbench, relay CLI (`list` / `look` /
  `raww` / `send` / `tui`), MCP bridge, tmux / ACP transports, and
  multi-worktree association.

[Unreleased]: https://github.com/emcd/agentmux/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/emcd/agentmux/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/emcd/agentmux/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/emcd/agentmux/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/emcd/agentmux/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/emcd/agentmux/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/emcd/agentmux/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/emcd/agentmux/releases/tag/v0.2.0
