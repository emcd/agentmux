# Change: Pty transport (libghostty-vt, in parallel with Tmux)

## Why

`agentmux` ships its coder-side delivery on top of `tmux`. Today every
coder-backed session is a tmux pane; the relay's `TmuxTransport` invokes
`tmux send-keys`/`capture-pane`/`new-session` to mediate between the
relay and the child's terminal. That has been the only option since
the project started, and it has accumulated sharp edges:

- **External-process dependency.** The `tmux` binary must be on `PATH`
  on every contributor machine and every CI runner. The relay cannot
  start coder sessions on systems without it, and the project README's
  "Requirements" section calls this out as the lone hard prerequisite.
- **Process as state.** Tmux is a long-lived system process outside
  the agentmux process tree. Its lifetime exceeds the relay's by
  design: a tmux session can survive an agentmux restart, but agentmux
  has no view into a tmux session that *predates* it, no native way to
  observe the session beyond the `tmux` CLI, and no way to be sure that
  a tmux session is "ours" versus an operator's stray session.
- **A second binary = a second parser.** The relay parses tmux's text
  CLI output (`capture-pane`, `display-message -p`) to recover cursor
  position, prompt-readiness status, and screen content. Any tmux
  version bump that rewraps its output is a potential regression.
- **Wedged-pane detection is best-effort.** The merged
  `tmux-wedge-detection` proposal added a three-state classifier
  (running/unresponsive/wedged) on top of `capture-pane` diffs. The
  classifier works, but it is bounded by what tmux makes observable.

`libghostty-vt` is the VT-engine extracted from Ghostty (the terminal
emulator used by millions of daily active users), with safe Rust
bindings (`libghostty-vt = "0.2.0"`, MIT OR Apache-2.0). A spike
(throwaway `src/bin/agentmux_pty.rs`, validated end-to-end via
`pty-debug`) confirmed the binding round-trips a real shell prompt,
key encoder, and effect handlers through a portable-pty child PTY.
The transport-trait surface (`src/transports/contract.rs`) already
forward-declares `TransportImpl::Pty` with the capability row
`look=true, write=true, stream=true, choices=false`, so the seam
exists; only the implementation is missing.

This proposal lands Pty as a **parallel, opt-in transport** so coders
can be configured under either `[coders.<id>.tmux]` or
`[coders.<id>.pty]`. Tmux is preserved. Pty is the recommended path
going forward once the new transport proves itself in real workloads;
cutover (defaulting new coders to Pty, then retiring Tmux) is sequenced
as a follow-up proposal after Pty matures. The alpha-software
defaults make no-backcompat claims permissible in principle, but
opting to default to an unproven transport is reckless when the
proven one exists and works — the "alpha" exception is about not
preserving uncertain past decisions, not about ripping out the
working thing for the unproven thing.

## What Changes

- Land a new `PtyTransport` in `src/pty/transport.rs` that implements
  the existing `Transport` trait in `src/transports/contract.rs`. The
  transport owns one `libghostty_vt::Terminal`, one `portable_pty`
  master, one reader thread, and one delivery task. The terminal is
  `!Send + !Sync`; the transport keeps it on the delivery thread and
  reaches the look path through a `Arc<Mutex<PtyState>>` shared view
  handle.
- Replace the `TransportImpl::Pty` unit variant forward-declared at
  `src/transports/contract.rs:199` with `TransportImpl::Pty(PtyTransport)`.
  Remove the `unimplemented!("PTY transport not yet implemented")`
  guards from each dispatch arm (`startup`, `mailw`, `raww`, `is_ready`,
  `shutdown`, `give_output`) and wire them to the new implementation.
  Add a `TransportImpl::pty(target_member, batch_settings)` constructor
  mirroring `tmux(batch_settings)`.
- Add a `[coders.<id>.pty]` per-coder config table in `coders.toml`
  (parallel to the existing `[coders.<id>.tmux]` table). v1 fields:
  - required: `initial-command`, `resume-command`
  - optional: `prompt-regex`, `prompt-inspect-lines`,
    `prompt-idle-column`, `cols` (default 120), `rows` (default 40),
    `prime-timeout-ms`, `wedge-detection` (default `true`)
- Pty adds the `SessionType::Pty` variant to the runtime session type
  taxonomy. The existing capability row in `session-relay` (look=true,
  write=true, stream=true, choices=false) becomes a real, populated
  row rather than a forward-looking note. Bundle session entries
  resolve to `Pty` when the referenced coder defines `[coders.<id>.pty]`
  instead of `[coders.<id>.tmux]`.
- The `Prompt-Readiness Template Gating` requirement in
  `session-relay` is extended to Pty: the same `prompt_regex` /
  `inspect_lines` / `input_idle_cursor_column` knobs apply. The wedge
  detection and prime timeout knobs (`prime-timeout-ms`,
  `wedge-detection`) are likewise extended to Pty with the same
  semantics as Tmux (per the merged `tmux-wedge-detection` proposal),
  with one structural simplification: the
  `operator_interaction_active` concept is **dropped** for Pty.
- Add `src/pty/` module with `transport.rs` (the transport impl),
  `state.rs` (the `Arc<Mutex<PtyState>>` shared state and output view),
  and a `PtyQuiescenceProbe` adapter for the shared wedge/prime state
  machine. The wedge/prime state machine itself lives in
  `src/transports/quiescence.rs` (the cross-transport home, alongside
  `contract.rs`, `ui.rs`, and `vocabulary.rs`), generalized over a
  `WedgeProbe` trait and lifted from
  `src/tmux/transport.rs::wait_for_quiescent_pane_three_state`. Tmux's
  three-state classifier continues to operate; the new generalized
  form is the shared machine that both transports use. The Tmux
  transport constructs a small `TmuxAsWedgeProbe` adapter that maps
  the existing `PaneQuiescenceProbe` into the new generalized trait,
  preserving the 16-probe test surface in
  `tests/unit/tmux_transport.rs` unchanged.
- The `WorkerReadinessState` surface in `transport-abstraction` adds
  Pty as a second populator alongside ACP. Pty's worker thread
  publishes `Available` on successful `startup`, `Busy` while a flush
  group is in flight, `Unavailable` on transport-failure outcomes
  (`pane_wedged` / `Timeout`), and `Recovering` when the transport is
  respawned after a child exit. ACP's existing readiness wiring is
  unchanged.
- Add `libghostty-vt = "0.2.0"` (default-features = false; we skip
  `kitty-graphics` to keep the build surface minimal) and
  `portable-pty = "0.9.0"` as workspace dependencies in `Cargo.toml`,
  gated behind a default-off `pty` Cargo feature (see Decision:
  Feature-gate Pty to keep default builds Zig-free). When the
  feature is off, the `src/pty/` module is not compiled, the
  `TransportImpl::Pty` variant is the existing unit-variant stub,
  and the dispatch arms fall through to `unimplemented!(...)` —
  preserving today's behavior.
- Update `README.md`'s "Requirements" section to note that Pty users
  additionally need Zig 0.15.x on `PATH` (the vendored build of
  `libghostty-vt` runs `zig build`); document the network-free
  override escape hatches (`GHOSTTY_SOURCE_DIR`,
  `GHOSTTY_ZIG_SYSTEM_DIR`) for sandboxed CI. The "Tmux on PATH"
  requirement remains unchanged (Tmux is still supported in
  parallel). The `agentmux-pty` POC binary (`src/bin/agentmux_pty.rs`)
  moves from "throwaway spike" to "kept-as-an-operator-tool" (still
  not wired into the relay or `Transport` trait) so operators can
  smoke-test Pty sessions end-to-end before configuring a bundle;
  the bin target itself is gated on the `pty` feature.

## Non-goals

- **Cutover / deprecation of Tmux.** Both transports ship side by
  side. Tmux remains the recommended default for v1. A follow-up
  OpenSpec proposal (sequenced after this one proves out in real
  workloads) will flip the default and eventually retire Tmux. This
  proposal does NOT change the Tmux transport at all (except to lift
  the shared wedge/prime logic out of `src/tmux/transport.rs` so Pty
  can reuse it without duplicating the state machine).
- **Removing the existing `[coders.<id>.tmux]` config surface.** Tmux
  coder config continues to load and validate unchanged. Existing
  bundles continue to work.
- **External tmux-attach replacement.** The operator-attach workflow
  ("open another shell and `tmux attach -t <session>`") is intentionally
  NOT replicated for Pty. `agentmux look <session>` is the v1
  replacement. A future proposal MAY add a publish-the-PTY-master RPC
  for external-debugger workflows; not in scope here.
- **Multi-viewer dimension auto-detection.** Pty's clean resize story
  makes it possible to drive the child's grid from a connected TUI's
  advertised viewport, but this requires a multi-viewer story
  (what happens when CLI `look` and TUI viewer with different dims
  are both attached) before it is real design work. Recorded as
  Future Work; not designed in this proposal.
- **ACP changes.** ACP delivery is in-process JSON-RPC, not a
  terminal. This proposal does not touch `src/acp/` and does not
  introduce a Pty↔ACP cross-cutting concern. The ACP wedge companion
  proposal (`acp-prime-timeout-and-wedge-detection`) consumes the
  `DeliveryEnvelope.prime_timeout_ms` field shape that
  `tmux-wedge-detection` already introduced; that work is
  independent of this proposal.

## Impact

- Affected specs:
  - `transport-abstraction` — Pty becomes a populated transport,
    `TransportImpl::Pty` becomes `Pty(PtyTransport)`,
    `WorkerReadinessState` adds a second populator.
  - `session-relay` — Pty adds a new `SessionType` row (replacing
    the forward-looking note in the capability contract); per-coder
    config gains `[coders.<id>.pty]` table; `Prompt-Readiness
    Template Gating` is extended to Pty; the wedge detection /
    prime timeout / quiescence-gated delivery requirements gain
    Pty-side scenario coverage.
- Affected code:
  - `Cargo.toml` — add `libghostty-vt` and `portable-pty` dependencies
    gated behind a default-off `pty` Cargo feature
    (`pty = ["dep:libghostty-vt", "dep:portable-pty"]`); add the
    new bin target `agentmux-pty` gated on the same feature.
  - `src/lib.rs` — add `#[cfg(feature = "pty")] pub mod pty;` (the
    `src/pty/` module is only compiled when the feature is enabled).
  - `src/transports/contract.rs` — replace `TransportImpl::Pty` unit
    variant with `#[cfg(feature = "pty")] Pty(PtyTransport)` (the
    `#[cfg(not(feature = "pty"))] Pty` unit-variant stub is
    preserved for default builds); add `pty()` constructor (cfg'd);
    delete the `unimplemented!` guards for Pty across all dispatch
    arms (the cfg arms fall through to `unimplemented!` when the
    feature is off).
  - `src/transports/mod.rs` — re-export `PtyTransport` (cfg'd);
    add `pub mod quiescence;` (the new shared wedge/prime state
    machine module is compiled unconditionally because the Tmux
    transport imports it).
  - `src/transports/quiescence.rs` (new) — the generalized
    `WedgeProbe` trait and the lifted three-state classifier.
    Compiled unconditionally.
  - `src/pty/` (new module, gated on `pty` feature) — `transport.rs`,
    `state.rs`, the `PtyQuiescenceProbe` adapter, `mod.rs`.
  - `src/tmux/transport.rs` — extract the three-state classifier and
    the prime-timeout / wedge-detection state machine into the
    shared `src/transports/quiescence.rs`. Tmux's transport impl
    calls into the shared machine via a small `TmuxAsWedgeProbe`
    adapter; the `PaneQuiescenceProbe` trait stays in
    `src/tmux/transport.rs` because it has 16 unit-test probes
    that are not worth porting.
  - `src/configuration/types.rs` — add `PtyTargetConfiguration` to
    the per-coder config; mirrors `TmuxTargetConfiguration` shape
    (required `initial-command`/`resume-command`; optional
    prompt-readiness; per-target `cols`/`rows`; per-target
    `prime_timeout_ms` and `wedge_detection`).
  - `src/configuration/raw.rs` and `targets.rs` — mirror the new
    fields through the raw loader and validator. Validator rejects
    `prime-timeout-ms = 0` for Pty, same as Tmux.
  - `src/relay/handlers/sender.rs` — construct `PtyTargetConfiguration`
    for Pty-backed sessions (alongside the existing
    `TmuxTargetConfiguration` construction).
  - `src/relay/delivery/quiescence.rs` — generalize `QuiescenceOptions`
    to take an optional `prime_timeout_ms` (already done by
    `tmux-wedge-detection`; this proposal consumes the existing
    field).
  - `tests/unit.rs` — register `tests/unit/pty_transport.rs`.
  - `tests/unit/pty_transport.rs` (new) — at minimum the same
    behavior classes as Tmux's 16-probe surface (AlwaysUnresponsive,
    AlwaysWedge, PendingChoice, SlowPrompt, NormalFlow), mapped to
    Pty's three-state classifier with the `PtyQuiescenceProbe` test
    seam.
  - `README.md` — Requirements section gains the Zig note; CLI
    Surface section documents the parallel Pty coder config; MCP
    Surface section notes that Pty sessions use the same look
    bounds as Tmux.

## Decision: Pin libghostty-vt tightly; revisit per release

Alpha-software default: do not preserve backwards compatibility
unless explicitly requested. libghostty-vt is pre-1.0
(`0.2.0` released 2026-06-16); the upstream docs explicitly warn
"the API is not yet stable, breaking changes are expected in future
versions." The transport-decoupling direction (`Transport` trait,
`DeliveryEnvelope.prime_timeout_ms`) gives us a single seam where
breaking libghostty-vt API changes would land — `src/pty/transport.rs`
is the only module that imports `libghostty_vt::*`. Operators
track upstream by re-vendoring per release; breaking changes are
expected to be contained to that one module.

## Decision: Feature-gate Pty to keep default builds Zig-free

The Pty transport depends on `libghostty-vt` and `portable-pty`.
The libghostty-vt vendored build runs `zig build` at build time
and clones ghostty source from network if `GHOSTTY_SOURCE_DIR`
is unset. Requiring Zig on `PATH` and network access at build time
for every contributor and CI run is a significant cost — and it
applies even to lanes that never touch Pty (CLI, MCP, ACP-only
bundles, TUI). The proposal is "Pty is opt-in per-coder" at the
bundle-config level; that opt-in story must carry through to the
build level too.

Therefore:

- A default-off `pty` Cargo feature gates both deps
  (`pty = ["dep:libghostty-vt", "dep:portable-pty"]`).
- The `src/pty/` module is gated on `#[cfg(feature = "pty")]`
  in `src/lib.rs`. The new bin target `agentmux-pty` is gated on
  the same feature.
- The `TransportImpl::Pty` variant is `#[cfg(feature = "pty")]
  Pty(PtyTransport)`; without the feature it stays the existing
  unit-variant stub, and the dispatch arms (`startup`/`mailw`/
  `raww`/`is_ready`/`shutdown`/`give_output`) carry cfg-gated
  fall-through arms that resolve to today's `unimplemented!(...)`
  when the feature is off. Default builds behave exactly as today.
- The shared wedge/prime state machine in
  `src/transports/quiescence.rs` is compiled unconditionally
  because the Tmux transport (which is always built) imports it.

CI implications (tracked as a follow-up task — see `tasks.md`
section 12): the existing `.github/workflows/tester.yaml` lint job
runs `cargo clippy --all-targets --all-features -- -D warnings`,
which activates every feature and would require Zig on the lint
runner when the `pty` feature is enabled. The test job has no
`--all-features`, so Pty gets zero CI coverage without an explicit
matrix entry. The follow-up task either installs Zig in the lint
runner + adds a `pty`-feature matrix entry to the test job, or
excludes the `pty` feature from `--all-features` (the latter
loses Pty coverage in CI; not recommended). The CI adjustment is
NOT in scope for this proposal's implementation tasks; it lands
as `todos/relay/98` immediately after the proposal merges.

## Decision: Drop `operator_interaction_active` for Pty

In the Tmux wedge-detection proposal, `operator_interaction_active`
suppresses both classifiers while the operator is interacting with
the pane (copy-mode / key-table introspection). For Pty, there is no
operator-attached TUI: the relay-tui consumer is read-only (`look`
snapshots), and `agentmux raww` is just an envelope/raw-bytes write
that does not change the agent's prompt-readiness state in a way
that needs classifier suppression. Therefore:

- `operator_interaction_active` is always `false` for Pty.
- The wedge state machine for Pty has TWO states (running /
  wedged-or-unresponsive), not three (running / unresponsive /
  wedged-with-operator-pause).
- The `operator_interaction_active` clause in the
  `Prompt-Readiness Template Gating` requirement's
  wedge-detection scenario is dropped for Pty scenarios (kept for
  Tmux scenarios, which still need it).

This is a meaningful simplification of the spec. It is NOT a
regression — the operator-interaction concept was a defensive
mechanism for a multi-actor terminal topology that Pty does not
have.

## Amendment history

- **Revision 1 (post-Coordinator-review, folded via `--amend`):**
  1. **Cargo feature-gating.** The original draft added
     `libghostty-vt` and `portable-pty` as plain `[dependencies]`,
     which would require Zig on `PATH` and network access at
     build time for every contributor and CI run, regardless
     of whether they touched Pty. The proposal now gates both
     deps and the `src/pty/` module and bin target behind a
     default-off `pty` Cargo feature. The `TransportImpl::Pty`
     variant carries cfg-gated alternative forms: when the
     feature is on, `Pty(PtyTransport)`; when off, the existing
     unit-variant stub. Dispatch arms carry cfg-gated fall-through
     arms. CI implications (Zig on the lint runner + Pty-feature
     matrix entry in the test job) are tracked as a follow-up
     task in `tasks.md` section 12 (`todos/relay/98`).
  2. **Module placement.** The original draft placed the
     generalized wedge/prime state machine in
     `src/pty/quiescence.rs`. The state machine is shared
     machinery (Tmux adapts into it via `TmuxAsWedgeProbe`), not
     Pty-specific. The revision moves it to
     `src/transports/quiescence.rs`, alongside `contract.rs`,
     `ui.rs`, and `vocabulary.rs` — the cross-transport home.
     The Pty module keeps a `PtyQuiescenceProbe` adapter that
     implements the shared `WedgeProbe` trait.
- **Revision 2 (post-RG-review, folded via `--amend`):**
  1. **POC artifact aligned with proposal's feature-gating
     contract.** The POC commit was made before the Revision 1
     feature-gating decision landed, so it added
     `libghostty-vt` and `portable-pty` as unconditional
     dependencies and exposed `agentmux-pty` without
     `required-features`. The amendment brings the artifact
     into alignment: both deps are now `optional = true` with
     the `pty` feature gating them, the `[features]` table is
     added with `pty = ["dep:libghostty-vt", "dep:portable-pty"]`,
     and `agentmux-pty` carries `required-features = ["pty"]`.
     `cargo metadata --no-deps` confirms both deps are now
     optional; `cargo build` succeeds without the feature and
     does not pull in libghostty-vt; `cargo build --features
     pty --bin agentmux-pty` succeeds and runs the Zig vendored
     build.
  2. **POC binary's module-level doc updated.** The binary's
     doc comment was rewritten from "throwaway POC, SPIKE code,
     NOT intended for merge to master" to "operator smoke-test
     entry point for the libghostty-vt binding, gated behind
     the default-off pty Cargo feature," aligning with the
     proposal's framing of the artifact as a kept-as-operator-
     tool deliverable rather than a spike.
  3. **Exact-version pin encoded correctly.** The design's
     "pin tightly (no `^`)" Decision is now encoded as
     `version = "=0.2.0"` (Cargo's exact-pin syntax), not the
     caret-compatible `version = "0.2.0"` that was in the
     Revision 1 task snippet. `cargo metadata` confirms the
     resolved dependency request is `=0.2.0`. `portable-pty`
     remains caret-compatible (`"0.9.0"` = `^0.9.0`) because the
     wezterm-maintained crate is more stable and the proposal
     does not require exact pinning for it.
  4. **Out-of-scope bullet clarified.** The "Pty cutover / Tmux
     deprecation" out-of-scope bullet was reworded. The
     original wording ("New bundles default to
     `[coders.<id>.pty]`; old bundles migrate; eventually
     Tmux is retired") read as if cutover were part of the
     proposal's deferred scope, but conflicted with the v1
     statement that "Tmux remains the recommended default for
     v1" (proposal §Why). The revised wording makes it
     explicit that cutover is a SEPARATE follow-up proposal,
     not v1 scope.

## Out of scope (deferred to follow-up proposals)

- **Pty cutover / Tmux deprecation.** A separate follow-up
  proposal (NOT this one) flips the default for new bundles to
  `[coders.<id>.pty]`, migrates existing bundles, and eventually
  retires Tmux. Sequenced after Pty proves itself in real
  workloads; not part of this proposal's scope.
- **ACP wedge companion.** `acp-prime-timeout-and-wedge-detection`
  is independent of this proposal; ACP owns its own wedge/prime
  state machine, parallel to Tmux/Pty.
- **External-attach RPC.** Future RPC for publishing the PTY master
  so external debuggers can attach. Useful but not in scope.
- **Multi-viewer dimension auto-detection.** Once Pty is the default,
  a TUI-viewport-driven resize story becomes possible. Requires a
  multi-viewer design first.
- **Vendored-artifact pipeline.** A side project that builds
  libghostty-vt once with Zig and republishes pre-built static
  libs per platform, removing the Zig dep for agentmux contributors.
  Sequenced post-cutover (see Future Work in `design.md`).

## Validation plan

- `cargo test --lib` and `cargo test --tests` pass with no regressions.
- `cargo clippy --all-targets --no-deps` is silent.
- `cargo fmt --check` is silent.
- `openspec validate add-pty-transport --strict` is valid.
- `cargo run --bin agentmux-pty -- /bin/bash` round-trips a real
  shell prompt through libghostty-vt (existing POC validation;
  preserved as an operator smoke-test entry point).
- Pty delivery on a Pty-configured coder session in a real bundle
  delivers a `mailw` envelope to a child shell and resolves
  `SendOutcome::Delivered` within the configured `quiet_window`.
- Pty `look` returns a `LookSnapshotPayload::Lines { snapshot_lines }`
  consistent with the captured screen (round-trip test against
  `cat /etc/hostname` or equivalent).
- Pty wedge detection fires on a wedged pane with default-on config.
- Pty prime timeout fires on an unresponsive pane when configured.
- Pty wedge-disabled + prime-timeout-set scenario matches the
  Tmux spec's `Scenario: Tmux prime timeout bounds post-quiescence
  wait when wedge is disabled`.