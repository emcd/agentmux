## Context

Pty transport landed early in 0.9.0 as a feature-gated (`--features pty`) transport. Two gaps were deferred to 0.10.0 and tracked as `issues/runtime/8` (lazy spawn panics) and `issues/runtime/9` (held bundle still spawnable). Live reproduction on 2026-08-20 with `--features pty` confirms both are reachable and that the first is now a pre-release blocker:

- **Blocking probe:** `src/pty/state.rs:122`/`128` (`PtyPromptProbe::observe` — `blocking_send` at `:122`, `blocking_recv` at `:128`) and `src/pty/state.rs:71`/`75` (`PtyOutputView::look` — `fn look` at `:71`, `blocking_send` at `:75`) do `mpsc::Sender::blocking_send` + `oneshot::Receiver::blocking_recv` on the worker’s channels. The probe is reached on **every** Pty delivery via `run_async_delivery_worker` (`src/relay/delivery/dispatch/worker.rs:144,177`) → `submit_batch:519` (comment `508-511` says nothing awaits in between) → `gate_target:927` → `transport.is_ready_for_handover():837` → `PtyTransport::is_ready_for_handover` (`src/pty/transport.rs:972`) → `observe` — not just the lazy path. A healthy eager worker is the case that reaches the probe (early-return at `transport.rs:973-977` only short-circuits an absent/exited/non-`Available` worker), and the gate retries from the poll arm, so it re-panics per tick. The relay survives (tokio isolates worker panics), the child (`opencode --session …`) is spawned at `transport.rs:416` before the handshake and is then killed by `StartupGuard::Drop` as the panic unwinds, leaving an orphaned child and a permanently stranded delivery (`relay.send.async.queued` with no `completed`) — correctly observed live via `journalctl`. The panic fix is group 1 alone; eager parity does not remove it.

- **Eager parity gap:** `src/relay/lifecycle.rs:475` has an explicit stopgap: `TargetConfiguration::Pty(_) => Ok(())` — “the full Pty bootstrap path lands alongside the bootstrap-side refactor … not implemented in this commit”. `startup_bundle`/`reconcile_loaded_bundle`/`startup_member` thus never construct a Pty transport eagerly, unlike Tmux (`startup_tmux_member` eagerly) and ACP (persistent worker per `transport-contracts` 523-536). The first delivery via `build_worker_transport` in `relay/delivery/dispatch/envelope.rs` then lazily constructs the Pty transport. That lazy construction leaves `issues/runtime/9` (held bundle still in catalog) reachable for Pty, but it is **not** the cause of the panic — fixing the probe (group 1) is the whole blocker fix.

- **Held bundle:** `BundleCatalog::is_held` is only checked in `watcher.rs:308` (reconcile; definition at `catalog.rs:162`). `lookup` succeeds for a held bundle, so `resolve_target_bundle`/`ensure_bundle_group` can build a delivery group for a bundle the relay is deliberately not running, and `build_worker_transport` will then spuriously build a Pty member for it — currently masked by `issues/runtime/8`’s panic, but reachable once that is fixed.

Current state of `transport-contracts/spec.md`: only `Pty Default Per-Coder Dimensions` exists for Pty; no `Pty Persistent Worker Lifecycle` requirement. `runtime-bootstrap` spec describes bundle startup ordering but not Pty-specific wiring.

Stakeholders: Pty Specialist (transport owner, proposal author), BE (reviewer, owns the diagnostic groundwork for cli/16 and the `is_dir` probe fix), Coordinator (integrator, owns the joint live-session schedule blocked on this).

## Goals / Non-Goals

**Goals:**
- Fix the `blocking_recv` panic so a `send`/`raww`/`look` to a Pty member never blocks a tokio worker thread, and surface construction failures instead of holding a member through a dwell as `unreachable` — this is the entire 0.9.0 blocker fix (group 1), independent of eager startup.
- Bring Pty to eager-startup parity with Tmux and ACP: Pty workers are initialized during the bring-up pass, never lazily created by request handlers, and torn down wherever the runtime is torn down — worth doing on its own merits, but not load-bearing for the panic (group 2 is separable).
- Decide and encode the held-bundle semantics for delivery (reject vs allow-if-up-never-spawn) once, at group construction, so `issues/runtime/9` is closed or has its own guard test.

**Non-Goals:**
- Vendored-artifact pipeline or Pty feature build plumbing (already tracked, `todos/pty/6` DEFER).
- Changing the ACP worker model or Tmux lifecycle (already correct, used as the reference).
- Adding new transport types or changing the `Transport` trait shape beyond what the Pty lifecycle needs.

## Decisions

**1. Panic fix — make the probe non-blocking in `state.rs`; keep `start_transport_off_runtime` for startup.**

*Options:* (a) `spawn_blocking` at the caller (`is_ready_for_handover` in `worker.rs:837`), (b) make the handshake genuinely non-blocking in `src/pty/state.rs` (e.g., `async send`/`await` + `recv`/`await` or a sync channel that does not require `blocking_*`).

*Choice:* (a) for the `startup` handshake is already done (`start_transport_off_runtime` at `envelope.rs:93-110` wraps `Transport::startup` in `spawn_blocking` because the contract states `startup` is synchronous and therefore *any* implementation is a blocking call — already propagated at `envelope.rs:215-220`). For the *probe* (`PtyPromptProbe::observe:122/128` and `PtyOutputView::look:71/75`), the only call site in `src/relay/` is `worker.rs:837` (`gate_target` → `is_ready_for_handover`), reached via `submit_batch:519` (comment `508-511` says nothing awaits in between) on every batch submission to every Pty target. `submit_batch` is `sync` holding `&mut TransportImpl` plus `&mut` borrows of the handover window, activity marker, and inflight maps — lifting the readiness read into `spawn_blocking` would require restructuring the gate. That makes (b) — an async handshake in `state.rs` — substantially cheaper than (a), the reverse of the earlier ordering. Prefer (b); fall back to (a) only if the non-blocking conversion would require widening the `Transport` trait.

*Outcome:* the non-blocking conversion **did** require widening `Transport::is_ready_for_handover` to `async fn` (contract.rs, five impls and three test files) behind `#[allow(async_fn_in_trait)]`. This is the exact fallback trigger named above. The widening was taken deliberately: restructuring `submit_batch`'s borrows for `spawn_blocking` was more invasive than the trait change, and the probe timeout (B1) bounds the await. The design records the divergence here rather than falling back. `async_fn_in_trait` futures have no `Send` bound by default; `run_async_delivery_worker` is `spawn`ed so its future must be `Send`. Every impl's future is `Send` today (Pty's `!Send` `Terminal` never crosses its `await`), and the trait's `allow` is documented in contract.rs with that justification.

*Why not per-transport judgment:* envelope.rs’s `start_transport_off_runtime` already documents this: deciding per session type “pty spawns a process so it goes off-runtime, tmux only spawns threads so it stays” reasons from today’s implementation rather than the contract, and the next blocking implementation inherits a wrong decision.

*Alternative considered:* widening the `Transport` trait to make `startup` async — rejected as a larger contract change for a 0.9.0 blocker, and `spawn_blocking` already satisfies the synchronous contract without trait churn.

**2. Eager startup — wire Pty like Tmux/ACP, but only with a handoff; otherwise defer.**

*Problem:* `WorkerTransportSource` (`worker.rs:81-86`) has two arms: `Acp(AcpWorkerBootstrap)` — how ACP’s eagerly-initialized worker reaches delivery — or `Direct(WorkerTransportContext)`, where the worker builds its own `TransportImpl` (`worker.rs:184-190`). Pty is `Direct`. Constructing a `PtyTransport` in `lifecycle.rs` (as task 2.1 originally proposed) with nothing adopting it double-spawns: one `portable-pty` child from bring-up, a second from the first delivery, violating the spec’s “one worker per target session” (`spec.md:9`). `PtyMirrorStateFn` mirrors readiness state, not the handle, and the headline test `list principals` `ready: true` after `up` would go green on the broken implementation.

*Choice:* Two paths satisfy the dissolve trigger — either (a) scope a `WorkerTransportSource::Pty(PtyWorkerBootstrap)` handoff analogous to `AcpWorkerBootstrap` and make `run_async_delivery_worker` adopt the eagerly built handle, with tasks scoped accordingly, or (b) **defer** the eager Pty construction until such a seam exists, recording the double-spawn reason. Given B1 removes the blocker justification for this group (the panic is group 1 alone, every Pty delivery via `worker.rs:837`), deferring is defensible and is the smaller change for this 0.9.0 blocker. This proposal defers the eager wire-up: `lifecycle.rs:475` stays `Ok(())` for this change, the `Pty Persistent Worker Lifecycle` spec delta is landed as *deferred* (or as a follow-up change), and the lazy `build_worker_transport` path remains the only Pty construction path until the handoff exists.

*Why this model if not deferred:* it would reuse the proven ACP lifecycle shape and the existing `envelope.rs` Pty construction — the lazy path would become the fallback for a member that appears mid-session, but the normal hosted-bundle path would no longer need it. The decision to defer avoids shipping a double-spawn that the headline test cannot catch.

*If scoped:* the handoff carries the eagerly built `TransportImpl::Pty` handle (not just `PtyMirrorStateFn` state) from `lifecycle.rs` into `worker.rs`’s `Direct` branch, and `startup_member`/`startup_members` report `StartupFailureRecord` for Pty so `ready_session_count`/`failed_startups` reflect reality. Teardown mirrors ACP’s “wherever the runtime is torn down” once the worker exists eagerly.

**3. Held bundle — check `HostingIntent::Hold` once at group construction, reject.**

*Choice:* `issues/runtime/9`’s suggested location is `src/relay/handlers/send.rs:883` (`ensure_bundle_group`) and `src/relay/handlers/routed.rs:85` (`resolve_target_bundle`) via `catalog.rs:162`/`watcher.rs:308`: a single `catalog.is_held(namespace)` check before building the group, shared by every transport. If held, **reject** as `unavailable` — most consistent with hold’s meaning and with the fail-closed rule for missing catalog entries, and the spec delta now asserts it normatively (`spec.md:44` “the send resolves as unavailable (held)”). The check is *once* and *before* any `build_worker_transport` call. This also closes the Pty-specific held spawn hole without making Pty special, and it must land **with** group 1 (today it spawns then panics; after group 1 it would spawn and succeed — a silent violation).

*Alternative considered:* per-transport hold checks — rejected because membership in the catalog is not the same claim as being run, and any future transport that spawns in `startup` would inherit the same hole.

## Risks / Trade-offs

- **Probe fix is the whole blocker; eager parity is separable:** The panic reaches every Pty delivery via `worker.rs:837`, not just the lazy path, so group 1 alone resolves the 0.9.0 blocker. Eager Pty startup (group 2) does not contribute to it and, as written without a handoff, double-spawns. → Mitigation: defer group 2 until a `WorkerTransportSource::Pty` handoff exists, or scope that handoff explicitly; do not couple the blocker to eager parity.

- **Startup failure surfacing (if eager lands) changes `up`/`reloaded` outcomes:** Eager Pty startup will report `failed_startups` where it previously reported `ready: true` unconditionally. A bundle that previously looked `hosted` with zero failures may now report `degraded`/`failed` if its Pty coder definition is broken. → Mitigation: the `Ok(())` stopgap hid real failures; surfacing them is the intended correction. The `transport-contracts` delta will state the startup sequence and failure codes explicitly so callers can distinguish `pty_worker_spawn_failed` from `not_implemented`.

- **Probe off-runtime adds latency (if `spawn_blocking` chosen):** Moving the probe off the worker adds a `spawn_blocking` hop per handover check. → Mitigation: the async handshake in `state.rs` (option b) avoids the hop entirely and is preferred given `submit_batch`’s borrow shape; either way the probe is polled at a bounded cadence and the extra cost is a single dispatch, not a new timer, and the alternative is a panic.

- **Feature-gate interaction:** Pty code is `cfg(feature = "pty")`; the non-pty build must still compile and the `SessionType::Pty => Err(internal_unexpected_failure)` arm in `envelope.rs:222-227` must remain for that build. → Mitigation: the eager bring-up path is also `cfg`-gated, and the spec delta will note the feature gate explicitly.

- **Orphaned children from the old panic window:** The live reproduction left orphaned `opencode` children (pid 289202 and its `context7-mcp`/`nb-mcp`/`pty-mcp-server` children) even after relay restart. → Mitigation: the panic fix removes the window; existing orphans are flagged for operator cleanup and not killed unilaterally.

## Migration Plan

1. Land the `transport-contracts` delta — **only** `Pty Prompt Probe and Look Shall Not Block a Tokio Worker Thread` (group 1, the blocker) and `Held Bundle Deliveries Shall Not Spawn Workers` (group 3, must land with group 1 — otherwise group 1 unmasks `runtime/9` from loud panic to silent hold violation). `Pty Persistent Worker Lifecycle` (eager startup) is **not** in this delta; deferred to a follow-up that scopes the `WorkerTransportSource::Pty` handoff. No runtime data migration, spec-only.
2. Land the code: `state.rs` probe non-blocking (group 1, the blocker) and `is_held` guard at `handlers/send.rs:883`/`routed.rs:85` (group 3, via `catalog.rs:162`/`watcher.rs:308`); `lifecycle.rs` eager Pty bring-up stays `Ok(())` for this change (deferred). `envelope.rs` already propagates `startup` errors at `215-220` — no change needed there.
3. Rebuild with `--features pty` (Zig 0.15.x) and cold-restart the relay.
4. Verify: `send`/`raww` to a Pty member no longer panics (`relay.send.async.queued` → `completed`, no `tokio-runtime-worker` panic); `look` path is still gated by `registry.rs:582-586` (`get_output_view` returns `None` for Pty) so the `acp_worker_unavailable` stale code remains a follow-up, not a verification step for this change. `todos/pty/9` 11.6/11.7 remain blocked until eager parity lands.
5. Rollback: revert the change; the panic returns for **all** Pty delivery paths (not just the lazy path) — no data to migrate, but every Pty `send` again deadlocks the worker. Eager parity, if deferred, simply stays lazy+`Ok(())`.

## Open Questions

- *(Deferred — for the follow-up that lands `Pty Persistent Worker Lifecycle`)* Exact queue bounds and shutdown sequence if the handoff is scoped — mirror ACP’s `pending_max = 64` and 4-step shutdown, or tailor for `portable-pty`’s `ExitStatus`/`Child` handle? Probably mirror ACP for parity, but worth confirming against `src/pty/transport.rs`’s actual worker/reader thread shape.

Resolved: held-bundle guard *rejects* (spec ADDED `Held Bundle Deliveries Shall Not Spawn Workers` scenario asserts “the send resolves as unavailable (held)”) — `issues/runtime/9` preference for reject, and task 3.1 / `handlers/send.rs:883` / `routed.rs:85` implement it, so OQ2 is closed.
