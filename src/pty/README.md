# src/pty/

Pty transport: libghostty-vt-backed delivery with portable-pty child process
management. Compiled only when the `pty` Cargo feature is enabled; the default
`cargo build` does NOT pull libghostty-vt or portable-pty and does NOT invoke
Zig.

Build-time prerequisites (Zig 0.15.x on PATH, optional outbound network access
for the vendored ghostty clone) and the upstream escape hatches
(`GHOSTTY_SOURCE_DIR`, `GHOSTTY_ZIG_SYSTEM_DIR`, `libghostty-vt-sys/pkg-config`)
are documented in `documentation/development/README.md` Zig-free Pty Builds.

## Module layout

- `command` — tokenization helpers for the per-coder `initial_command`.
- `delivery` — the worker-thread delivery state machine. The Pty worker is
  the only thread that can apply bytes to the libghostty-vt terminal (the
  terminal is `!Send + !Sync`); the state machine drives a per-tick wait
  that drains PTY output during quiescence so the probe observes fresh
  terminal state. Each classify step supplies the target namespace and the
  current coalesced group's message ids to shared progress diagnostics; raw
  waits use an empty message-id group.
- `state` — cross-thread shared state (`PtyShared`, `PtyConfigSnapshot`,
  `SnapshotRequest`/`SnapshotResponse`) plus the per-thread look / probe
  consumers (`PtyOutputView`, `PtyQuiescenceProbe`).
- `transport` — `PtyTransport` (the per-target `Transport` implementation
  with its worker thread, delivery task, and reader thread) plus
  `PtyTargetConfiguration` (the per-coder config bundle).

## Terminal-outcome receipt rendering

Terminal-outcome receipts (relay/system-originated envelopes carrying
the outcome of a prior async send, routed to the original sender through
the sender's own Pty transport) are rendered with a leading marker line
so the receiving agent can distinguish them from peer messages at a
glance. The marker line is emitted only for receipt envelopes; peer
envelopes render unchanged. Detection uses the
`DeliveryEnvelope.is_receipt` field the relay propagates from the
receipt builder — see `build_coder_envelope` in
`src/relay/delivery/dispatch/envelope.rs`. Receipts are non-recursive
at the relay's terminal-resolution chokepoint; the Pty transport does
not enforce or check that invariant.

The marker line and the rendered pane envelope are written
contiguously under the same `writer.lock()` so the marker and the
envelope cannot be interleaved with another write on the same Pty
master. Within a coalesced group every receipt gets its own marker
line; peer envelopes that coalesce beside a receipt render normally.

## Per-coder TERM protocol

The Pty transport reads `[coders.<id>.pty].term-protocol` to set the
`TERM` env var when spawning the child (default `xterm-256color`). The
full enum and config wiring live in `src/configuration/types.rs`; the
unit-test coverage is in `tests/unit/pty_transport.rs` (the
`#[ignore]`-gated `pty_transport_term_protocol_propagates_to_child_command`
and `pty_transport_term_protocol_dependent_round_trip_through_snapshot`
tests cover both the TERM propagation and the TERM-dependent-behavior
round-trip through the snapshot path). The smoke binary
(`src/bin/agentmux_pty.rs`) accepts a `--term-protocol <value>` CLI flag
mirroring the same kebab-case strings the TOML config field uses, so an
operator can smoke-test the same TERM value a coder is configured with
without editing the bundle config.

## Cross-references

- `src/transports/contract.rs` — the transport contract and the
  `DeliveryEnvelope` / `DeliveryMessage` types the relay populates and
  the Pty transport renders.
- `src/transports/quiescence.rs` — the cross-transport wedge/prime
  quiescence state machine consumed by `delivery.rs` and the other
  coder transports.
- `src/configuration/types.rs` — `TermProtocol` enum and
  `PtyTargetConfiguration` definitions.
- `src/envelope.rs` — the canonical pane-envelope renderer
  (`render_envelope`) and `AddressIdentity` helpers.
- `src/relay/delivery/async_worker.rs` — the relay-side terminal-
  resolution chokepoint that builds receipts (`complete_task_outcome` →
  `deliver_terminal_outcome_receipt` → `build_terminal_outcome_receipt`)
  and the non-recursion enforcement at the single spawn site.
- `documentation/development/README.md` — Pty build prerequisites, Zig
  escape hatches, and the upstream `libghostty-rs` `pkg-config` PR gap.
