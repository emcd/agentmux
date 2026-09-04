# src/pty/

> **Work-in-progress — not production-ready.** The Pty transport landed early in
> the 0.9.0 cycle, but it is marked explicitly WIP. This change
> (`fix-pty-eager-startup-and-probe-blocking`) fixes `agentmux:issues/runtime/8`
> (Pty prompt probe `blocking_recv` panic in a tokio worker) and
> `agentmux:issues/runtime/9` (held-bundle guard — `is_held` at group
> construction), both previously milestone 0.10.0. Remaining deferred gap:
> eager Pty startup parity (`Pty Persistent Worker Lifecycle` — requires a
> `WorkerTransportSource::Pty` handoff analogous to `AcpWorkerBootstrap`,
> `worker.rs:81-86` vs `Direct` at `184-190`; see `lifecycle.rs:475` stopgap).
> This module is compiled only when the `pty` Cargo feature is enabled.

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
- `delivery` — `PtyDeliveryWriter`, this transport's half of the shared
  delivery-loop executor. The worker thread is the only one that can apply
  bytes to the libghostty-vt terminal (the terminal is `!Send + !Sync`), so the
  terminal lives there and the loop comes to it. Readiness is evaluated against
  that terminal directly rather than through the snapshot channel, because this
  thread is the one that would answer such a request. Every entry is its own
  packing unit — one entry per plan, buffered and written as one primitive — so
  each member's evidence is its own write's result and no group-wide outcome is
  applied. `wait_for_work` is overridden so the terminal keeps being fed and
  keeps answering snapshot requests while the executor is idle.
- `state` — cross-thread shared state (`PtyShared`, `PtyConfigSnapshot`,
  `SnapshotRequest`/`SnapshotResponse`) plus the look consumer
  (`PtyOutputView`) and the prompt-readiness predicate (`prompt_satisfied`).
- `transport` — `PtyTransport` facade (struct, `Transport`/`GenerationFence` impls, `PtyTargetConfiguration`); `transport::lifecycle` owns bring-up (`startup_inner`, `StartupGuard`, bounded `observe_thread_finished`); `transport::runtime` owns the `!Send` terminal worker/reader threads (`run_worker`, `run_reader`, handlers, snapshot render, `publish`). `delivery` remains `src/pty/delivery.rs` (worker-thread `Delivery` state machine).

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

A receipt is **declared and acknowledged like any other entry**. It
bypassed relay admission, so it holds no quota reservation — but it does
take a mailbox position, and under the pull model that is what makes it
deliverable at all: an executor writes only what it peeks, so an entry
with no position is one nothing would ever write. The only thing that
follows from the missing reservation is that its acknowledgment releases
none. Pty needs no special case for it beyond the marker line below. See
`src/relay/delivery-architecture.md`, "Members no reservation covers".

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
- `src/transports/diagnostics.rs` — the transport-neutral delivery-progress
  inscription context.
- `src/configuration/types.rs` — `TermProtocol` enum and
  `PtyTargetConfiguration` definitions.
- `src/envelope.rs` — the canonical pane-envelope renderer
  (`render_envelope`) and `AddressIdentity` helpers.
- `src/relay/delivery/async_worker/` — the relay-side terminal-
  resolution chokepoint (`terminal.rs`, `complete_task_outcome`) and the
  receipt construction it hands off to (`reporting.rs`,
  `deliver_terminal_outcome_receipt` → `build_terminal_outcome_receipt`),
  including the non-recursion enforcement at the single spawn site.
- `documentation/development/README.md` — Pty build prerequisites, Zig
  escape hatches, and the upstream `libghostty-rs` `pkg-config` PR gap.
