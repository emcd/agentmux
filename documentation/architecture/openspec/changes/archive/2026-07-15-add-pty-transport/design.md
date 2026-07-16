# Design: Pty transport (libghostty-vt, in parallel with Tmux)

## Context

`agentmux` is a multi-agent coordination runtime. The relay host
process mediates messaging between agent sessions; each session is
backed by a transport that owns one child process and surfaces a
look/write stream to the relay. Three transports exist today:
ACP (in-process JSON-RPC), Tmux (out-of-process tmux server
managing a pane per session), and Ui (relay stream broadcast for
read-only TUI subscribers).

Tmux has been the only coder-side transport since the project
began. The merged `tmux-wedge-detection` proposal added bounded
prime-wait and wedge-classifier behavior to the Tmux transport's
quiescence wait, addressing two failure modes (unresponsive target
/ wedged pane) that the original unbounded wait could not
distinguish from healthy agent behavior.

`libghostty-vt = "0.2.0"` is the VT-engine extracted from Ghostty,
shipped with safe Rust bindings by `github.com/uzaaft/libghostty-rs`
(licensed MIT OR Apache-2.0). A throwaway POC
(`src/bin/agentmux_pty.rs`, validated end-to-end via `pty-debug`)
confirmed the binding round-trips a real shell prompt, key encoder,
and effect handlers through a portable-pty child PTY. The
`Transport` trait in `src/transports/contract.rs` already
forward-declares `TransportImpl::Pty` with the capability row
`look=true, write=true, stream=true, choices=false`. The seam exists;
only the implementation is missing.

This proposal is the formal design for the Pty transport landing
in parallel with Tmux, the config surface that exposes it as a
per-coder opt-in, and the spec deltas that update
`transport-abstraction` and `session-relay` to recognize it as a
first-class transport rather than a forward-looking note.

## Goals / Non-Goals

### Goals

- Land a fully-functional `PtyTransport` that passes the same
  behavior-class unit tests as Tmux's wedge detector.
- Add `[coders.<id>.pty]` per-coder config as a peer of
  `[coders.<id>.tmux]`.
- Make the transport-decoupling direction (`Transport` trait,
  `DeliveryEnvelope.prime_timeout_ms`) the canonical seam for
  prime-timeout / wedge-detection logic across all coder
  transports, not just Tmux.
- Drop `operator_interaction_active` for Pty (and only for Pty)
  with rationale captured in the spec delta.
- Document the Zig build dependency and the network-free override
  escape hatches in `README.md` and (post-implementation) in
  `AGENTS.md`.

### Non-Goals

- Cutover / deprecation of Tmux. Both transports ship side by side.
- Removing the existing `[coders.<id>.tmux]` config surface.
- External tmux-attach replacement (publish-the-PTY-master RPC).
- Multi-viewer dimension auto-detection.
- Vendored-artifact pipeline (a side project that pre-builds
  libghostty-vt with Zig and republishes static libs per platform).
- ACP changes — ACP delivery is in-process JSON-RPC; Pty is purely
  a Tmux replacement.

## Decisions

### Decision: Pin libghostty-vt tightly; revisit per upstream release

`libghostty-vt = "0.2.0"` is pre-1.0. The upstream docs explicitly
warn "the API is not yet stable, breaking changes are expected in
future versions." We pin to the exact version (no `^`), and
operators track upstream by re-vendoring per release.

The transport-decoupling direction gives us a single seam where
breaking upstream API changes would land: `src/pty/transport.rs`
is the only module in the project that imports `libghostty_vt::*`.
Upstream breakage is expected to be contained to that one module.

If the upstream API breaks in a way that does not cleanly fit the
single-module containment, we will either:

1. Pin a fork with backported fixes, or
2. Add an internal wrapper trait (e.g. `CrateGhosttyVt`) that
   abstracts the binding surface and absorbs upstream churn.

We document this trade-off in `src/pty/transport.rs`'s module
doc-comment so future maintainers see it.

Alternatives considered:

- Wait for upstream 1.0. Rejected: the wedge-detection limitation
  on Tmux (text CLI parsing) is the operational pain point driving
  this work; waiting many months for upstream 1.0 means operators
  eat wedge-detection false-positives in the interim.
- Wrap libghostty-vt in an internal trait from day one. Rejected
  as premature: the safe Rust wrapper is already pretty thin and
  the trait would be 90% pass-through. We do this only if upstream
  churn actually materializes.

### Decision: Feature-gate Pty to keep default builds Zig-free

The Pty transport's only mandatory build-time addition is Zig
0.15.x on `PATH` (for the libghostty-vt vendored build). The
project today has no build-time toolchain dep beyond `cargo` and
the standard rust toolchain. Adding Zig as a global requirement
— which the original draft did implicitly by listing
`libghostty-vt` and `portable-pty` as plain `[dependencies]` — is
unacceptable: contributors running `cargo build` on a lane that
never touches Pty (CLI, MCP, ACP-only bundles, TUI) would still
need Zig and would still trigger a network clone of ghostty at
build time. The "opt-in per-coder" story at the bundle-config
level must carry through to the build level too.

Therefore:

- A default-off `pty` Cargo feature gates both deps:
  `pty = ["dep:libghostty-vt", "dep:portable-pty"]`. The default
  `cargo build` / `cargo test` does not pull in libghostty-vt or
  portable-pty and does not invoke Zig.
- The `src/pty/` module is gated on `#[cfg(feature = "pty")]`
  in `src/lib.rs`. When the feature is off, the module is not
  compiled.
- The new bin target `agentmux-pty` is gated on the same feature.
- The `TransportImpl::Pty` variant is cfg-gated:
  `#[cfg(feature = "pty")] Pty(PtyTransport)` when on, and
  `#[cfg(not(feature = "pty"))] Pty` (the existing unit-variant
  stub) when off. Dispatch arms (`startup`/`mailw`/`raww`/
  `is_ready`/`shutdown`/`give_output`) carry cfg-gated alternative
  patterns: when the feature is on, the arm delegates to the
  `PtyTransport` method; when off, the arm falls through to
  today's `unimplemented!(...)`. Default builds behave exactly
  as today.
- The shared wedge/prime state machine in
  `src/transports/quiescence.rs` is compiled unconditionally
  because the Tmux transport (always built) imports it.

CI implications: the existing `.github/workflows/tester.yaml`
lint job runs `cargo clippy --all-targets --all-features -- -D
warnings`, which activates every feature. When the `pty` feature
is enabled, the lint runner needs Zig on `PATH` (and network
access for the ghostty clone, or the override env vars set).
The test job has no `--all-features`, so Pty gets zero CI
coverage without an explicit matrix entry. The CI adjustment is
tracked as a follow-up task (`tasks.md` section 12,
`todos/relay/98`): install Zig in the lint runner + add a
`pty`-feature matrix entry to the test job. The follow-up lands
immediately after this proposal merges; not in this proposal's
implementation scope.

Alternatives considered:

- Always-on deps with Zig as a hard build requirement. Rejected:
  opts everyone in to the Zig dep, including lanes that never
  touch Pty. The cost is not justified by Pty's
  opt-in-per-coder design.
- Optional deps without the feature flag (`[dependencies.optional]
  = true`) plus a feature flag. Cargo's optional-deps syntax
  requires the feature anyway; the explicit `dep:libghostty-vt`
  form is the same end result and is the conventional cargo idiom
  for this pattern.

### Decision: Vendored Zig build for v1; vendored-artifact pipeline post-cutover

The `libghostty-vt-sys` crate's default `vendored` feature runs
`zig build` at build time, cloning ghostty source from network if
`GHOSTTY_SOURCE_DIR` is unset. This adds two operational costs:

- **Build-time Zig dep.** `zig 0.15.x` must be on `PATH` for any
  contributor building the project. We already have Zig 0.15.2 on
  `PATH` via `mise`.
- **Build-time network I/O.** Without `GHOSTTY_SOURCE_DIR`, the
  build script clones ghostty from GitHub at build time. CI/sandbox
  builds need overrides.

For v1, we accept both costs and document them:

- Add Zig 0.15.x to `AGENTS.md`'s Prerequisites section.
- Add the two network-free escape hatches
  (`GHOSTTY_SOURCE_DIR`, `GHOSTTY_ZIG_SYSTEM_DIR`) to the README's
  build-from-source section so sandboxed CI can use them.
- Enable `libghostty-vt-sys/pkg-config` as an opt-in feature flag
  for users who want to point at a prebuilt `libghostty-vt.a`
  installed via pkg-config. Not enabled by default.

Post-cutover (after Pty is the recommended transport and upstream
stabilizes), we revisit owning a vendored-artifact pipeline: a
side project that builds libghostty-vt once with Zig and republishes
pre-built static libs per platform, removing the Zig dep for
agentmux contributors entirely. Sequencing this post-cutover is
correct because pre-1.0 churn makes a rebuild-and-republish cycle
expensive during exactly the period we are least sure of the API.

Alternatives considered:

- Adopt Zig as an official toolchain dep and pin it via Nix now.
  Rejected: we're not setting up CI yet, and we'd be designing CI
  infrastructure speculatively while validating the binding.
- Upstream the build escape hatch (`GHOSTTY_SOURCE_DIR` only).
  The escape hatch already exists upstream; we just document it.

### Decision: Drop `operator_interaction_active` for Pty

In Tmux, the `operator_interaction_active` flag suppresses both
the wedge classifier and the prime-timeout classifier while an
operator is in copy-mode or scrolling through the pane. The
motivation was: "if the operator is intentionally making the pane
static, the agent's prompt-readiness is not the relevant signal."

For Pty, there is no operator-attached TUI. The relay-tui consumer
is read-only (`look` snapshots); `agentmux raww` is just an
envelope or raw-bytes write that does not change the agent's
prompt-readiness state in a way that needs classifier suppression.

Therefore:

- `operator_interaction_active` is always `false` for Pty.
- Pty's wedge state machine has two terminal states (`running` and
  `wedged-or-unresponsive`), not three.
- The spec delta modifies the `Prompt-Readiness Template Gating`
  requirement's wedge-detection scenarios to keep the
  `operator_interaction_active` clause for Tmux scenarios and drop
  it for Pty scenarios, with an explicit rationale.

This is a meaningful simplification of the spec. It is NOT a
regression — the operator-interaction concept was a defensive
mechanism for a multi-actor terminal topology that Pty does not
have. The Tmux concept remains correct for Tmux and is not
disturbed.

Alternatives considered:

- Approximate `operator_interaction_active` for Pty via DEC mode 1
  + DEC mode 1049 heuristics (alt-screen / cursor-key
  application). Rejected: those heuristics detect "the child is
  in some full-screen TUI mode," which is a different signal than
  "the operator is interacting with the pane." Conflating them
  would be wrong.
- Keep the same three-state machine for Pty with
  `operator_interaction_active` always false. Rejected: dead code
  in the state machine; better to acknowledge the simplification
  in the spec.

### Decision: Pty spawns at per-coder default dims; runtime resize is future work

v1 config knobs under `[coders.<id>.pty]`:

- `cols` (default 120)
- `rows` (default 40)

Pty spawns the child at those dims. `portable_pty` handles the
Winsize struct: the configured `PtySize` is supplied to
`openpty` (which sizes the master + slave pair at open time);
`portable_pty` then propagates the dimensions to the slave's
`TIOCSWINSZ` ioctl when the child is spawned via `spawn_command`.
`look()` returns `LookSnapshotPayload::Lines { snapshot_lines }`
from `Formatter::format_alloc(Format::Plain)`, truncated to the
consumer's `LookMode.lines` (existing field).

> **Spec-alignment note (2026-07-16):** the prior text said
> "we call `terminal.resize(cols, rows, 0, 0)` once at startup";
> the shipped implementation does NOT call `terminal.resize`
> (the terminal is constructed at the configured dimensions via
> `Terminal::new(TerminalOptions { cols, rows, max_scrollback:
> 10_000 })` and the PTY pair's Winsize is set at openpty /
> spawn time). The resize path is reserved for a future
> `agentmux resize <session> <cols> <rows>` command.

Runtime resize is NOT in v1. There is no operator-facing
`agentmux resize <session> <cols> <rows>` command. If we add it
later, the implementation is mechanical: call
`terminal.resize(...)` AND `pty.Setsize(...)` (TIOCSWINSZ).

The default 120 x 40 matches the POC and modern terminal sizes.
The 80 x 24 default that Tmux uses was a tmux-server default, not
a load-bearing constraint; we pick the default that matches the
POC and operator preference.

Alternatives considered:

- Make the relay-tui consumer drive the child's grid from its
  advertised viewport. Rejected for v1: requires a multi-viewer
  story (what happens when CLI `look` and TUI viewer with
  different dims are both attached) before it is real design
  work. Recorded as Future Work.
- Mirror Tmux's 80 x 24 default for compat. Rejected: there is no
  compatibility constraint (config is opt-in; the user picks
  defaults). Modern terminal sizes are larger.

### Decision: Generalized wedge/prime state machine in `src/transports/quiescence.rs`

Today the wedge detection and prime-timeout state machine lives
in `src/tmux/transport.rs::wait_for_quiescent_pane_three_state`.
The 16-probe unit test surface in `tests/unit/tmux_transport.rs`
exercises it.

For Pty we want the same state machine, but with a probe trait
shaped for Pty's primitives (`RenderState::dirty()`, cursor
position via `terminal.cursor_x/y()`, prompt-readiness regex match
against `Formatter::format_alloc` output) instead of tmux's
primitives (`capture-pane`, `display-message -p`).

The state machine is shared cross-transport machinery — both Tmux
and Pty consume it — so it lives in `src/transports/quiescence.rs`
alongside `contract.rs`, `ui.rs`, and `vocabulary.rs` (the
cross-transport home). It is generalized over a `WedgeProbe`
trait that abstracts "what is the current state of this quiescence
wait": probe → renders the inspected tail (Pty or Tmux), probe →
checks prompt-readiness, probe → reports whether the target is
settled, probe → reports whether operator interaction is active.

The Tmux transport keeps its existing `PaneQuiescenceProbe` trait
in `src/tmux/transport.rs` because its 16 unit-test probes are not
worth porting. The Tmux transport constructs a small adapter
(`TmuxAsWedgeProbe`) that maps the existing probe into the new
generalized trait, so the state machine itself is shared.

The Pty transport defines a parallel `PtyQuiescenceProbe` adapter
in `src/pty/state.rs` that implements the shared `WedgeProbe`
trait using Pty-specific primitives (the `Terminal`, the
formatter, the prompt-readiness template). The generalized state
machine operates over it directly.

Both transports populate the same shared wedge/prime outcomes
(`SendOutcome::Timeout`, `SendOutcome::Failed` +
`reason_code = "pane_wedged"`).

The `src/transports/quiescence.rs` module is compiled
unconditionally — even when the `pty` Cargo feature is off —
because the Tmux transport (always built) imports it.

Alternatives considered:

- House the shared state machine in `src/pty/quiescence.rs`.
  Rejected: the module would be Pty-owned but Tmux would depend
  on it, which reads incorrectly in the module tree (Tmux
  depending on a Pty module). The cross-transport home is the
  natural fit.
- Duplicate the state machine for Pty. Rejected: the wedge/prime
  semantics are transport-agnostic by design (per the
  `tmux-wedge-detection` proposal); duplication would mean two
  bugs to fix when semantics evolve.
- Make the Tmux `PaneQuiescenceProbe` trait generic over the
  primitives. Rejected: the trait is already exercised by 16 unit
  tests; making it generic would require touching all 16.

### Decision: Pty as `TransportImpl::Pty(PtyTransport)` with a worker thread

The Pty transport owns:

- One `libghostty_vt::Terminal<'static, 'static>` (must live on a
  single thread; the cross-thread look path goes through a
  `SnapshotRequest` channel whose receiver lives on the worker).
- One `portable_pty::PtyPair` master.
- One reader thread that loops on `pty_master.read() → channel →
  terminal.vt_write()`.
- One delivery task that processes the `mailw`/`raww` future
  resolution.
- One `Arc<PtyOutputView>` shared with the relay's look path.

The terminal is `!Send + !Sync`. The transport owns the terminal
and the delivery task; the delivery task itself spawns the
reader thread and owns the terminal directly. The relay worker
calls `mailw`/`raww` via the `Transport` trait — these methods
are sync on the trait surface (no `.await`) and enqueue onto an
internal `mpsc::Sender<DeliveryCommand>`. The delivery task
drains the channel and produces terminal output.

The shipped cross-thread coordination point for `look()` is a
`SnapshotRequest` channel (the receiver lives on the worker
thread; the look path sends a request, the worker renders the
snapshot from the live terminal and replies on a oneshot).
The look path never touches the terminal directly. The reader
thread forwards raw bytes through `bytes_tx` to the worker,
which applies them to the terminal and advances
`last_change_atomic` (shared with `PtyQuiescenceProbe`). The
`PtyShared` struct carries the cross-thread state (`config`,
`last_change_atomic`, `snapshot_tx` sender, `child_exited`).

> **Spec-alignment note (2026-07-16, Pty archive):** the
> proposal-draft design originally specified
> `Arc<Mutex<PtyState>>` with the look path locking the mutex
> directly. The shipped implementation uses a snapshot-request
> channel instead because:
> - `libghostty_vt::Terminal` exposes no safe way to lock
>   terminal state across threads without an Arc<Mutex<>>
>   wrapper, AND the worker thread that owns the terminal is
>   the only thread that can render snapshots — putting the
>   rendering on the worker (via the channel) avoids
>   serializing terminal access with a mutex held across
>   blocking I/O (which the libghostty-vt FFI doesn't allow).
> - The reader thread and the look path both produce
>   cross-thread input for the worker; channelizing both makes
>   the worker the single owner of all terminal mutations.

> The original `Alternatives considered` block (mutex path vs
> message-passing) was rewritten below to reflect the shipped
> design choice (message-passing) and the rejected mutex
> path's specific concerns.

The Pty worker populates the existing `WorkerReadinessState`
surface in `transport-abstraction`: `Available` on successful
`startup`, `Busy` while a flush group is in flight,
`Unavailable` on transport-failure outcomes (incl. wedge-class
resolutions) and on child exit. `Recovering` is NOT emitted by
this implementation today — it requires a respawn monitor that
the Pty transport does not yet have; the variant is preserved
in the live enum for future use, and the respawn-monitor follow-up
will add the emission path (deferred to the bootstrap-side
wiring follow-up).

> **Scope-amendment note (2026-07-16, Pty archive):** the prior text
> "`Recovering` when the transport is respawned after a child exit"
> described a transition this implementation cannot emit (no
> respawn monitor exists yet). The `WorkerReadinessState` enum
> retains `Recovering` for ACP + future Pty use; only the Pty
> emission claim is removed. The `Unavailable`-on-child-exit
> half of the latched-condition contract IS implemented (the worker
> observes EOF on the PTY master via `PtyShared.child_exited`,
> publishes `Unavailable`, abandons any in-flight delivery with a
> `pty_child_exited` `Failed` outcome, drains queued commands with
> the same `Failed` outcome, and refuses to publish `Available`
> again until restart).

Alternatives considered:

- Make the terminal thread-local to the delivery task and
  expose a separate API for look. Rejected: this would require a
  new mechanism for the look path to access terminal state and
  would diverge from the existing `OutputView` shape.
- Ship the original `Arc<Mutex<PtyState>>` design with the
  look path locking the mutex directly. Rejected: required
  the look path to acquire the terminal's mutex across the
  blocking `format_alloc` call, which serializes look
  throughput against any other code path that needs the
  terminal (the delivery task's wait-step snapshot path). The
  snapshot-channel design lets the worker own the terminal
  exclusively and serve both the wait-step probe and the look
  path from the same thread.

### Decision: Coder config layout — parallel tables, no per-call override

Per-coder shape:

```toml
[[coders]]
id = "codex"

[coders.pty]      # NEW: opt in to Pty transport
initial-command = "codex"
resume-command = "codex resume {coder-session-id}"
prompt-regex = "(?m)^›"
prompt-inspect-lines = 3
prompt-idle-column = 2
cols = 120         # NEW: per-coder grid defaults
rows = 40          # NEW
prime-timeout-ms = 30000
wedge-detection = true   # default; explicit opt-out

[[coders]]
id = "opencode"

[coders.tmux]     # EXISTING: unchanged
initial-command = "opencode"
# ...
```

The two tables are mutually exclusive at the bundle level: a
coder entry defines exactly one of `[coders.<id>.pty]` or
`[coders.<id>.tmux]`. The validator rejects both or neither.

No `[coders.<id>.acp]` change; ACP is its own table.

No per-call override. `agentmux send` and MCP `send` continue to
not accept per-call transport timeouts. Operators configure
`prime-timeout-ms` and `wedge-detection` per-coder under the
per-coder table. The rationale (mirroring the merged
`tmux-wedge-detection` proposal): per-call overrides multiply the
config surface, hide failure modes from the operator, and create
behavioral divergence across delivery attempts to the same target.

## Risks / Trade-offs

- **Pre-1.0 binding risk.** `libghostty-vt = "0.2.0"` may break in
  a future upstream release. Mitigation: tight version pin +
  single-module containment (see Decision: Pin libghostty-vt).
- **Build-time Zig dep.** Adds a build toolchain requirement for
  contributors. Mitigation: document the Zig requirement;
  document the network-free escape hatches for CI; defer the
  vendored-artifact pipeline to post-cutover.
- **`!Send + !Sync` constraint.** All `libghostty_vt` types must
  live on a single thread; cross-thread coordination requires
  message passing. Mitigation: a `SnapshotRequest` channel whose
  receiver lives on the worker thread; the look path sends a
  request, the worker renders the snapshot from the live
  terminal and replies on a oneshot, validated by the POC.
- **Network I/O at build time.** The vendored build clones ghostty
  from GitHub. Mitigation: `GHOSTTY_SOURCE_DIR` override; doc'd in
  the README.
- **No external attach.** Operators lose the `tmux attach` workflow.
  Mitigation: `agentmux look` is the documented replacement; future
  publish-the-PTY-master RPC is possible.
- **Multi-viewer dimension policy is deferred.** If a future
  proposal tries to drive the child's grid from the TUI's
  advertised viewport, it will hit a multi-viewer design problem.
  Mitigation: Future Work section explicitly calls this out.
- **Two coder-transport paths to maintain.** Pty and Tmux both
  exist in v1. Mitigation: the shared wedge/prime state machine
  in `src/transports/quiescence.rs` means the behavioral logic is
  in one place; the per-transport adapters are small.

## Migration Plan

There is no migration. Existing bundles continue to use
`[coders.<id>.tmux]` unchanged. New bundles MAY use
`[coders.<id>.pty]`. Operators can move one coder at a time by
editing their `coders.toml`.

The cutover proposal (sequenced after this one proves out in real
workloads) will:

1. Add a default per-coder policy that prefers Pty when the
   coder entry is silent on transport (currently no such default;
   the existing config requires explicit `[coders.<id>.tmux]` or
   `[coders.<id>.acp]`).
2. After a release cycle, change the existing tmux-wedge-detection
   spec requirement to recommend Pty in new bundles.
3. After another release cycle, deprecate Tmux and recommend
   migrating.
4. After another release cycle, retire Tmux (separate OpenSpec).

This proposal does NOT change any of those steps; it only lands the
opt-in path.

## Future Work

- **Vendored-artifact pipeline for libghostty-vt.** A side project
  that builds libghostty-vt once with Zig and republishes
  pre-built static libs per platform, removing the Zig dep for
  agentmux contributors entirely. Sequenced post-cutover because
  pre-1.0 churn makes a rebuild-and-republish cycle expensive.
- **Multi-viewer dimension auto-detection.** Once Pty is the
  default, a TUI-viewport-driven resize story becomes possible.
  Requires a multi-viewer design first: what happens when CLI
  `look` and TUI viewer with different dims are both attached to
  the same session? The clean resize story makes this easier to
  do right than the equivalent tmux design (which is why this
  Future Work item exists in the Pty design and not in Tmux).
  Concretely: Pty's resize story means the runtime resize API
  could be driven by TUI-viewport auto-detection rather than only
  manual `agentmux resize` once that API lands.
- **External-attach RPC.** A future RPC for publishing the PTY
  master so external debuggers can attach. Useful but not in
  scope here.
- **Pty cutover / Tmux deprecation.** Sequenced after Pty proves
  itself in real workloads.
- **ACP wedge companion.** `acp-prime-timeout-and-wedge-detection`
  is independent of this proposal.

## Open Questions

None at proposal time. All five questions raised during the
three-way discussion are resolved (recorded in the Coordinator
exchange archived at `artifacts/general/1`).

## Reference material

- Upstream libghostty-vt: `github.com/uzaaft/libghostty-rs`
  (also vendored at `~/src/THIRD_PARTY/libghostty-rs`).
- Reference Go implementation: `montanaflynn/headless-terminal`
  (vendored at `~/src/THIRD_PARTY/headless-terminal`) — the
  closest architectural analog for a daemon that owns PTY +
  libghostty-vt + look/snapshot.
- Original C libghostty-vt demo: `ghostty-org/ghostling`
  (vendored at `~/src/THIRD_PARTY/ghostling`).
- POC binary: `src/bin/agentmux_pty.rs` (kept as an operator
  smoke-test entry point after this proposal lands).
- Spike findings note:
  `.auxiliary/scribbles/libghostty-vt-exploration/findings.md`.
- Predecessor proposal: `tmux-wedge-detection` (merged; provides
  the `DeliveryEnvelope.prime_timeout_ms` field shape this
  proposal consumes).