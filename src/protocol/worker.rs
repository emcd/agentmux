//! Readiness and failure state for a persistent worker serving a delivery
//! target.

/// Readiness state for a persistent worker serving a delivery target.
///
/// Transport-agnostic: any worker-driven transport populates this state. ACP is
/// the only populator today; Pty is the next expected one.
///
/// The relay's per-target delivery workers mutate this state in an in-process
/// registry. Out-of-crate observers read it through the relay's
/// `subscribe_worker_readiness` subscription — today only the ACP integration
/// tests; the relay's own respawn/startup gating reads it internally. The TUI
/// does **not** consume this enum: it observes worker transitions as relay wire
/// stream events instead. The state is stringified by the relay rather than
/// serialized directly, so it carries no `serde` derive.
///
/// A failure cause is deliberately **not** a payload on `Unavailable`: it is
/// carried separately as a [`WorkerFailureReason`] (see that type for why the two
/// are orthogonal). Keeping this enum payload-free preserves its `Copy`-ness and
/// lets every producer signal `Unavailable` without a reason in scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerReadinessState {
    Initializing,
    Available,
    Busy,
    Recovering,
    Unavailable,
}

/// The most recent unrecoverable failure a delivery worker reported.
///
/// Captured so the relay startup path can surface the true cause (e.g. why an ACP
/// agent's `initialize` handshake failed) instead of a generic "worker
/// unavailable" placeholder.
///
/// This is intentionally a *separate* piece of state from [`WorkerReadinessState`]
/// rather than a payload on the `Unavailable` variant, because the two are
/// orthogonal:
///
/// - The failure must **outlive** the `Unavailable` state. A non-permanent
///   failure (e.g. a spawn failure) triggers respawn, which moves the worker to
///   `Recovering` on a backoff — yet the startup poller giving up during that
///   window still needs the original cause. A variant payload would vanish on the
///   `Recovering` transition; a separate field survives it.
/// - It is recorded by the two workers sites that *have* a structured cause (the
///   initial bootstrap and the permanent-respawn give-up), and **cleared** on any
///   healthy (`Available`/`Busy`) transition — so a recovered worker never
///   carries a stale reason. Producers of `Unavailable` with no cause in scope
///   (mid-turn teardown, connection-closed) simply do not touch it.
///
/// Transport-agnostic like [`WorkerReadinessState`]; ACP is the only populator
/// today. The `code`/`reason` mirror the transport's own structured bootstrap
/// error, so the relay carries them uninterpreted.
#[derive(Clone, Debug)]
pub struct WorkerFailureReason {
    /// A machine-readable failure code (e.g. `runtime_acp_initialize_failed`).
    pub code: String,
    /// A human-readable description of the failure cause.
    pub reason: String,
}
