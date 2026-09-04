use std::sync::Arc;

use serde_json::Value;

use crate::transports::{Chooser, OutputView, WorkerFailureReason, WorkerReadinessState};

/// Mirrors the worker readiness state into the relay's global registry.
pub type MirrorStateFn = Arc<dyn Fn(WorkerReadinessState) + Send + Sync>;
/// Records the worker's most recent unrecoverable failure into the relay
/// registry, so the startup path can surface its true cause behind an
/// `Unavailable` readiness state.
pub type RecordFailureFn = Arc<dyn Fn(WorkerFailureReason) + Send + Sync>;
/// Publishes the transport's `look` [`OutputView`] handle into the relay registry.
pub type PublishOutputFn = Arc<dyn Fn(Option<Arc<dyn OutputView>>) + Send + Sync>;
/// Broadcasts an ACP respawn stream event (`event_type`, `payload`) to the bundle UI.
pub type BroadcastUiFn = Arc<dyn Fn(&str, Value) + Send + Sync>;
/// Invalidates the target's pending operator choices before a respawn attempt.
pub type InvalidateChoicesFn = Arc<dyn Fn() + Send + Sync>;

/// Relay-provided lifecycle touchpoints, injected once when the driver is built.
///
/// Each closure closes over the relay's own registries/services for one target;
/// the driver holds opaque `Arc<dyn Fn>`s typed only in `transports`, so
/// `src/acp` never imports `crate::relay`.
#[derive(Clone)]
pub struct AcpDriverServices {
    /// Mirrors the worker readiness state into the relay's global registry (the
    /// TUI worker-state stream and the relay's own respawn gate observe it).
    pub mirror_state: MirrorStateFn,
    /// Records the worker's structured failure into the relay registry just
    /// before the `Unavailable` transition, so the startup poller reads the true
    /// cause (e.g. the ACP `initialize` failure reason) rather than a generic
    /// placeholder. Called only on unrecoverable failures.
    pub record_failure: RecordFailureFn,
    /// Publishes the transport's `look` [`OutputView`] handle into the relay
    /// look registry. Called before each `startup` so a `look` racing init finds
    /// the handle and runs its bounded prime-wait.
    pub publish_output: PublishOutputFn,
    /// Broadcasts an ACP respawn stream event (`event_type`, `payload`) to the
    /// bundle's registered UI sessions. The relay closure wraps it in its own
    /// `RelayStreamEvent`.
    pub broadcast_ui: BroadcastUiFn,
    /// Invalidates the target's pending operator choices before a respawn
    /// attempt, logging its own failure. Encapsulates the relay choice-queue
    /// context construction.
    pub invalidate_choices: InvalidateChoicesFn,
    /// Re-entrant operator-choice resolver threaded into every [`StartupContext`].
    pub chooser: Chooser,
}

impl std::fmt::Debug for AcpDriverServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpDriverServices").finish_non_exhaustive()
    }
}
