# Change: Generalize worker readiness into a transport-agnostic interface

## Why

The relay's per-target worker readiness surface is spelled ACP-specific
end to end, even though the state model is already transport-neutral. The
registry entry holds `acp_state: Option<AcpWorkerReadinessState>`, the
mutator/reader pair is `set_acp_worker_state` / `get_acp_worker_state`, the
in-process observer is `subscribe_acp_worker_state`, and the public read is
`read_acp_worker_state`. The `decouple-transport-layer` arc deliberately left
this naming alone (tasks 4.5/4.6/4.9): renaming the observer without also
generalizing the registry field and the enum would have been a half-measure,
so the holistic generalization was deferred to the point where a second
transport grows a multi-state readiness lifecycle. Pty is approaching that
point, and the ACP naming now obscures that readiness is a generic
worker-lifecycle concept rather than an ACP detail.

## What Changes

- **BREAKING (relay crate API):** Rename the public observer
  `read_acp_worker_state` to `read_worker_readiness`. Same return contract
  (`Option<&'static str>`, values `initializing`/`available`/`busy`/
  `recovering`/`unavailable`).
- **BREAKING (relay crate API):** Rename the readiness enum
  `AcpWorkerReadinessState` to `WorkerReadinessState` in
  `src/transports/vocabulary.rs`; the variants are already transport-neutral and
  are unchanged. The enum is re-exported as `crate::transports::AcpWorkerReadinessState`,
  so the rename breaks that public path.
- Rename the registry field `AsyncWorkerEntry.acp_state` to
  `AsyncWorkerEntry.readiness` and the relay-internal mutator/reader
  `set_acp_worker_state` / `get_acp_worker_state` to `set_worker_readiness` /
  `get_worker_readiness`.
- Rename the in-process observer surface
  `subscribe_acp_worker_state` / `publish_acp_worker_state` (and the
  `WORKER_STATE_PUBLISHERS` registry) to `subscribe_worker_readiness` /
  `publish_worker_readiness` / `WORKER_READINESS_PUBLISHERS`.
- Add a transport-abstraction requirement that names the worker-readiness
  interface (enum + per-target registry field + in-process observer + public
  read) as transport-agnostic, so any worker-driven transport (ACP today, Pty
  next) populates the same surface.
- **Non-goal:** This proposal does NOT add a second transport's readiness
  driver, does NOT change any wire-visible string (state values or look
  `stale_reason_code` codes), and leaves ACP's transition triggers and the
  ACP-look snapshot derivation untouched. See `design.md` for the scope
  boundary on the look `stale_reason_code` strings.

## Impact

- Affected specs:
  - `transport-abstraction` — ADDED: Worker Readiness Interface.
  - `session-relay` — MODIFIED: ACP Terminal Readiness Tracking (re-points the
    maintained state to the shared transport-neutral readiness registry; ACP
    triggers unchanged).
- Affected code:
  - `src/transports/vocabulary.rs` — enum rename + doc.
  - `src/relay/delivery/async_worker.rs` — `AsyncWorkerEntry.readiness`,
    `set_worker_readiness`, `get_worker_readiness`.
  - `src/relay/delivery/observability.rs` — observer/publisher rename,
    `WORKER_READINESS_PUBLISHERS`.
  - `src/relay/delivery/dispatch/worker.rs` — `mirror_state` closure call site.
  - `src/relay/mod.rs` — public `read_worker_readiness` re-export + stringify.
  - `src/transports/mod.rs`, `src/relay/contract.rs`,
    `src/relay/delivery/{mod.rs,dispatch/orchestration.rs}`,
    `src/acp/{state.rs,transport.rs,worker_driver.rs}` — type-name references.
  - Tests under `tests/` that name `read_acp_worker_state`,
    `subscribe_acp_worker_state`, or `AcpWorkerReadinessState`.
- Sequencing: overlaps `refactor-unified-namespace-registry` on
  `async_worker.rs` (`AsyncWorkerKey`/`AsyncWorkerEntry`) and
  `observability.rs`. Whichever lands first, rebase the other before applying;
  the two renames are orthogonal (namespace keying vs. readiness field).
- Related: capability-flag and `is_ready()` direction in `ideas/transport/4`;
  this is the
  holistic generalization the `decouple-transport-layer` 4.6/4.9 scope notes
  deferred.
