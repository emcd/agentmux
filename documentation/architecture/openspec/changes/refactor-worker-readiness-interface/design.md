## Context

The relay tracks per-target worker readiness in an in-process registry so the
delivery dispatch path can gate sends (busy/available), the respawn path can
detect failure (unavailable/recovering), and the look path can mark snapshots
stale while a worker is not ready. The state enum, the registry field, and the
observer surface are all spelled with an `acp`/`Acp` prefix today, even though:

- the variants (`Initializing`, `Available`, `Busy`, `Recovering`,
  `Unavailable`) describe a generic worker lifecycle, not ACP framing;
- the registry key (`AsyncWorkerKey`) is already per-target
  `(namespace, runtime_directory, target_session)`, and each target is served
  by exactly one transport; and
- the observer (`subscribe_acp_worker_state`) reads the relay's own registry,
  not anything ACP-wire-specific.

The `decouple-transport-layer` arc (tasks 4.5/4.6/4.9) removed the last
`crate::relay::AcpWorkerReadinessState` compat shim but deliberately kept the
ACP names, because renaming the observer alone — without the registry field and
the enum — would be a half-measure. This change performs that holistic rename
as forward-looking groundwork for a second worker-driven transport (Pty).

## Goals / Non-Goals

- Goals:
  - Rename the readiness enum, registry field, mutator/reader, in-process
    observer, and public read to transport-neutral names.
  - State in the spec that the worker-readiness interface is transport-agnostic
    and that ACP is one populator among future transports.
- Non-Goals:
  - Adding a second transport's readiness driver (Pty stays single-state until a
    separate change drives it). This is naming/contract groundwork only.
  - Changing any wire-visible string: the public read still returns the same
    five state strings, and the look `stale_reason_code` codes are unchanged.
  - Generalizing the ACP-look snapshot derivation (`derive_acp_look_snapshot`)
    or its `acp_*` stale-reason codes — see Decisions.
  - Changing ACP's transition triggers (stdin write success → busy, terminal
    stopReason → available, reader exit → unavailable). Those stay ACP-specific
    in `src/acp`.

## Decisions

- **Decision: rename to `WorkerReadinessState` and `*_worker_readiness`.** The
  enum becomes `WorkerReadinessState`; the relay-internal pair becomes
  `set_worker_readiness` / `get_worker_readiness`; the field becomes
  `AsyncWorkerEntry.readiness`; the observer becomes `subscribe_worker_readiness`
  / `publish_worker_readiness` over `WORKER_READINESS_PUBLISHERS`; the public
  read becomes `read_worker_readiness`. "Readiness" (not "state") is the precise
  noun — the registry already carries other per-worker state (sender, pending
  count, output view), so a bare `*_worker_state` name would over-claim.
  - Alternatives considered: `TransportReadinessState` (rejected — readiness is
    a property of the *worker* serving a target, not of the transport type, and
    one transport type drives many independent workers); keeping `acp_` and
    adding a parallel generic surface (rejected — two surfaces for one registry
    is exactly the half-measure 4.6 warned against).

- **Decision: one `readiness` field per entry, not a transport-keyed map.** The
  idea note phrased the target as "generic readiness keyed by transport." Because
  `AsyncWorkerKey` already identifies a single target served by a single
  transport, the entry holds one `Option<WorkerReadinessState>`; the "keyed by
  transport" property is satisfied implicitly by the worker key. An implementer
  SHALL NOT introduce a `HashMap<TransportKind, WorkerReadinessState>` — there is
  no per-entry multi-transport readiness to hold.

- **Decision: leave the look `stale_reason_code` strings ACP-prefixed and
  ACP-local.** `derive_acp_look_snapshot` (`src/acp/state.rs`) consumes the
  enum and emits `acp_worker_initializing` / `acp_worker_unavailable` /
  `acp_worker_recovering` / `acp_snapshot_prime_timeout`. These are wire-visible
  in look responses and the derivation is ACP-look-specific. Renaming the enum
  forces a type-reference touch in that function, but the function, its codes,
  and its ACP-look home stay as-is. When Pty grows a look path it will have its
  own derivation with its own codes; generalizing look-snapshot derivation is a
  separate change (related to the capability-flag direction in
  `ideas/transport/4`). Folding it in here would re-expand scope past the
  registry/enum/observer triad the idea note scoped.

## Risks / Trade-offs

- **Pure rename in alpha → low risk.** No backwards-compat shims are added
  (the decouple arc already removed the last one); call sites move atomically
  with the definitions in a single change. Pre-commit `cargo build`/clippy/tests
  catch any missed reference.
- **Overlap with `refactor-unified-namespace-registry`** on `async_worker.rs`
  and `observability.rs`. → Mitigation: the renames are orthogonal (namespace
  keying vs. the readiness field/observer); whichever proposal lands first, the
  other rebases onto local master before applying.
- **Spec churn on `session-relay`.** The MODIFIED requirement re-points the
  maintained state to the shared registry without changing ACP triggers or any
  scenario outcome, so behavior is unchanged; the edit is naming/anchoring only.

## Migration Plan

1. Rename the enum and field/observer/read symbols with their definitions.
2. Update all call sites (relay delivery, `src/acp`, `relay/mod.rs` re-export,
   `src/transports/mod.rs`) in the same change.
3. Update tests that name the old symbols.
4. `cargo build` + clippy `-D warnings` + full test run green before commit.
   No runtime data migration: the registry is in-process and ephemeral.

Rollback: revert the change; there is no persisted state keyed on these names.

## Open Questions

- None blocking. The look `stale_reason_code` generalization is intentionally
  deferred (see Decisions) and tracked alongside the per-transport look /
  capability-flag direction in `ideas/transport/4`.
