//! The ACP worker lifecycle driver.
//!
//! [`AcpWorkerDriver`] owns the per-target [`AcpTransport`] and its
//! bootstrap/respawn lifecycle. It is held by `TransportImpl::Acp`, so the relay
//! delivery worker drives ACP startup and recovery through the generic transport
//! handle without naming any ACP type. The driver depends only downward on
//! `crate::transports`, `crate::configuration`, and `crate::runtime` — never on
//! `crate::relay`.
//!
//! ## Relay touchpoints as injected closures
//!
//! The lifecycle reaches relay-owned registries (worker-state mirror, look
//! OutputView publish, choice-queue invalidation, UI stream broadcast) and the
//! relay choice queue (the [`Chooser`]). Each is injected as an opaque
//! `Arc<dyn Fn>` (or value) in [`AcpDriverServices`], constructed relay-side
//! closing over relay services; the driver invokes them without a back-edge,
//! mirroring the `Chooser` pattern from Slice 2b.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::configuration::BundleMember;
use crate::envelope::PromptBatchSettings;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    Chooser, DeliveryEnvelope, OutputView, StartupContext, Transport, TransportError,
    TransportStatus, WorkerFailureReason, WorkerReadinessState,
};

use super::{AcpBootstrapError, AcpTransport, bootstrap_acp_worker_runtime};

const RESPAWN_BACKOFF_MAX_MS_ENVVAR: &str = "AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS";
const RESPAWN_SLEEP_POLL_MS: u64 = 50;
const RESPAWN_BACKOFF_INITIAL_MS: u64 = 1_000;
const RESPAWN_BACKOFF_CAP_DEFAULT_MS: u64 = 30_000;
// How many times the same bootstrap failure may repeat before the respawn
// monitor gives up and settles the worker Unavailable. This is stage-agnostic:
// any non-permanent bootstrap failure (spawn, initialize, session/load,
// session/new, persist) counts, keyed on its (code, reason) signature. A
// deterministic failure that reproduces identically across this many
// backoff-spaced attempts is not going to self-resolve, so we surface it to
// operators rather than loop forever -- a genuinely transient failure that
// changes signature or succeeds within the window resets the counter and keeps
// recovering. Permanent failures (missing capability) short-circuit this via
// AcpBootstrapError::is_permanent before the count is even consulted. Before
// this was stage-agnostic, only repeated `initialize` failures could give up,
// so a coder whose session/load or session/new deterministically failed
// respawned forever and never published a terminal Unavailable.
const RESPAWN_REPEATED_FAILURE_THRESHOLD: u32 = 3;
/// Idle poll interval for the respawn monitor's shutdown gate.
const RESPAWN_MONITOR_POLL_MS: u64 = 100;
/// Generic respawn trigger label. The internal delivery task signals a boolean
/// respawn-needed edge (no reason code), so the monitor reports this for the
/// respawn stream events and inscriptions.
const RESPAWN_TRIGGER_REASON: &str = "worker_unavailable";

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

/// Owns the per-target ACP transport and its bootstrap lifecycle.
///
/// Held by `TransportImpl::Acp`. Delivery trait methods delegate to the inner
/// [`AcpTransport`] under a brief lock. The transport is shared with the
/// driver-owned async respawn monitor (spawned in [`bootstrap`](Self::bootstrap))
/// via `Arc<Mutex<…>>`, so respawn runs off the relay worker loop: the monitor
/// drives recovery while the worker keeps submitting writes. The monitor never
/// holds the lock across `.await` or the blocking child spawn, so a concurrent
/// `mailw` is never stalled.
pub struct AcpWorkerDriver {
    transport: Arc<Mutex<AcpTransport>>,
    namespace: String,
    runtime_directory: PathBuf,
    target_member: BundleMember,
    services: AcpDriverServices,
}

impl AcpWorkerDriver {
    /// Builds a driver for one ACP target with a fresh transport.
    #[must_use]
    pub fn new(
        target_member: BundleMember,
        runtime_directory: PathBuf,
        namespace: String,
        services: AcpDriverServices,
        batch_settings: PromptBatchSettings,
    ) -> Self {
        Self {
            transport: Arc::new(Mutex::new(AcpTransport::new(
                batch_settings,
                Some(Arc::clone(&services.mirror_state)),
            ))),
            namespace,
            runtime_directory,
            target_member,
            services,
        }
    }

    fn target_session(&self) -> &str {
        self.target_member.id.as_str()
    }

    /// Locks the shared transport. Locks are brief by construction (the respawn
    /// monitor never holds the lock across `.await` or the blocking child spawn).
    fn lock_transport(&self) -> std::sync::MutexGuard<'_, AcpTransport> {
        self.transport.lock().expect("acp transport mutex poisoned")
    }

    /// Establishes the ACP runtime when the worker starts and spawns the
    /// driver-owned respawn monitor. Mirrors the readiness transitions and
    /// publishes the look handle before bootstrap runs, so a `look` in the
    /// initial-startup window finds the handle. The blocking child spawn runs off
    /// the transport lock (via `spawn_blocking`); the fast install happens under a
    /// brief lock. On initial-bootstrap failure the transport is signalled for
    /// respawn so the monitor retries with backoff.
    pub async fn bootstrap(&mut self) {
        (self.services.mirror_state)(WorkerReadinessState::Initializing);
        let handle = self.lock_transport().give_output();
        (self.services.publish_output)(handle);

        // Set chooser/target identity ahead of the establish; the freshly-built
        // transport already reads Initializing from construction.
        self.lock_transport().prepare_for_startup(
            self.services.chooser.clone(),
            self.namespace.clone(),
            self.target_member.id.clone(),
        );

        let runtime_directory = self.runtime_directory.clone();
        let target_member = self.target_member.clone();
        let bootstrap_result = tokio::task::spawn_blocking(move || {
            bootstrap_acp_worker_runtime(&runtime_directory, &target_member)
        })
        .await
        .expect("ACP worker bootstrap task panicked");

        match bootstrap_result {
            Ok(runtime) => {
                self.lock_transport().install_runtime(runtime);
                (self.services.mirror_state)(WorkerReadinessState::Available);
            }
            Err(error) => {
                self.lock_transport().mark_runtime_unavailable();
                // Record the failure before the readiness transition so the
                // startup poller, which acts the moment it observes Unavailable,
                // finds the true cause already stored.
                (self.services.record_failure)(WorkerFailureReason {
                    code: error.code.clone(),
                    reason: error.reason.clone(),
                });
                (self.services.mirror_state)(WorkerReadinessState::Unavailable);
                emit_inscription(
                    "relay.acp.worker.bootstrap_failed",
                    &json!({
                        "namespace": self.namespace,
                        "target_session": self.target_session(),
                        "error_code": error.code,
                        "reason": error.reason,
                    }),
                );
                // No delivery task is running to emit the respawn-needed signal,
                // so prime it directly: the monitor will retry with backoff.
                self.lock_transport().signal_respawn();
            }
        }

        self.spawn_respawn_monitor();
    }

    /// Spawns the driver-owned async respawn monitor. It subscribes to the
    /// transport's stable respawn-needed signal and drives respawn off the relay
    /// worker loop, sharing the transport via `Arc<Mutex<…>>`.
    fn spawn_respawn_monitor(&self) {
        let transport = Arc::clone(&self.transport);
        let respawn_needed = self.lock_transport().respawn_needed_subscribe();
        let services = self.services.clone();
        let namespace = self.namespace.clone();
        let runtime_directory = self.runtime_directory.clone();
        let target_member = self.target_member.clone();
        tokio::spawn(acp_respawn_monitor(
            transport,
            respawn_needed,
            services,
            AcpRespawnState::new(),
            namespace,
            runtime_directory,
            target_member,
        ));
    }
}

impl Transport for AcpWorkerDriver {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        self.lock_transport().startup(context)
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        self.lock_transport().mailw(envelope)
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        self.lock_transport().raww(content, append_enter)
    }

    fn is_ready(&self) -> bool {
        self.lock_transport().is_ready()
    }

    fn can_accept_handover(&self) -> bool {
        self.lock_transport().can_accept_handover()
    }

    fn shutdown(&mut self) {
        self.lock_transport().shutdown();
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        self.lock_transport().give_output()
    }
}

/// Driver-owned async respawn monitor. Subscribes to the transport's stable
/// respawn-needed signal and drives respawn off the relay worker loop. The
/// transport is shared via `Arc<Mutex<AcpTransport>>`; the monitor locks only for
/// the fast release/install steps — never across `.await` or the blocking child
/// spawn — so a concurrent worker `mailw` is never stalled. Exits on relay
/// shutdown.
async fn acp_respawn_monitor(
    transport: Arc<Mutex<AcpTransport>>,
    mut respawn_needed: tokio::sync::watch::Receiver<bool>,
    services: AcpDriverServices,
    mut respawn_state: AcpRespawnState,
    namespace: String,
    runtime_directory: PathBuf,
    target_member: BundleMember,
) {
    let poll = Duration::from_millis(RESPAWN_MONITOR_POLL_MS);
    loop {
        tokio::select! {
            biased;
            changed = respawn_needed.changed() => {
                if changed.is_err() {
                    // All senders dropped: the transport is gone.
                    return;
                }
            }
            _ = tokio::time::sleep(poll) => {}
        }
        if shutdown_requested() {
            return;
        }
        if !*respawn_needed.borrow_and_update() {
            continue;
        }
        run_acp_respawn(
            &transport,
            &services,
            &mut respawn_state,
            namespace.as_str(),
            runtime_directory.as_path(),
            &target_member,
        )
        .await;
        // Reset the signal so a later Unavailable turn re-triggers the monitor.
        transport
            .lock()
            .expect("acp transport mutex poisoned")
            .clear_respawn_signal();
    }
}

/// Releases the dead runtime and re-establishes it with capped exponential
/// backoff, mirroring Recovering/Available/Unavailable transitions, broadcasting
/// respawn stream events, and invalidating pending choices before each attempt.
/// Returns when re-establish succeeds, the failure is permanent, the retry budget
/// is exhausted, or shutdown is requested. The blocking child spawn runs off the
/// transport lock; only the fast release/install steps hold it.
async fn run_acp_respawn(
    transport: &Arc<Mutex<AcpTransport>>,
    services: &AcpDriverServices,
    respawn_state: &mut AcpRespawnState,
    namespace: &str,
    runtime_directory: &Path,
    target_member: &BundleMember,
) {
    let target_session = target_member.id.as_str();
    // Release the dead runtime (joining its child + reader thread) but keep the
    // transport and its published handle, marking it Recovering. A look racing the
    // respawn reads a recovering/stale snapshot through the still-valid handle.
    transport
        .lock()
        .expect("acp transport mutex poisoned")
        .release_runtime();

    loop {
        if shutdown_requested() {
            return;
        }
        let backoff = respawn_state.advance();
        (services.mirror_state)(WorkerReadinessState::Recovering);
        emit_inscription(
            "relay.acp.respawn.triggered",
            &json!({
                "namespace": namespace,
                "target_session": target_session,
                "attempt": respawn_state.attempt,
                "trigger_reason": RESPAWN_TRIGGER_REASON,
                "backoff_ms": backoff.as_millis() as u64,
            }),
        );
        (services.broadcast_ui)(
            "acp_worker_respawn_started",
            json!({
                "attempt": respawn_state.attempt,
                "trigger_reason": RESPAWN_TRIGGER_REASON,
                "backoff_ms": backoff.as_millis() as u64,
            }),
        );

        if !sleep_with_shutdown_gate(backoff).await {
            return;
        }

        (services.invalidate_choices)();

        // Set chooser/target + clear the prior channel under a brief lock; the
        // chooser is already set from initial startup, re-set for safety.
        transport
            .lock()
            .expect("acp transport mutex poisoned")
            .prepare_for_startup(
                services.chooser.clone(),
                namespace.to_string(),
                target_member.id.clone(),
            );

        // Bootstrap the new runtime OFF the lock (blocking child spawn).
        let bootstrap_dir = runtime_directory.to_path_buf();
        let bootstrap_member = target_member.clone();
        let bootstrap_result = tokio::task::spawn_blocking(move || {
            bootstrap_acp_worker_runtime(&bootstrap_dir, &bootstrap_member)
        })
        .await
        .expect("ACP respawn bootstrap task panicked");

        match bootstrap_result {
            Ok(runtime) => {
                // Install under a brief lock; the published handle stays valid
                // (install repoints its replay slot).
                transport
                    .lock()
                    .expect("acp transport mutex poisoned")
                    .install_runtime(runtime);
                (services.mirror_state)(WorkerReadinessState::Available);
                emit_inscription(
                    "relay.acp.respawn.succeeded",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempt": respawn_state.attempt,
                    }),
                );
                (services.broadcast_ui)(
                    "acp_worker_respawn_completed",
                    json!({
                        "attempt": respawn_state.attempt,
                        "outcome": "succeeded",
                    }),
                );
                respawn_state.reset_on_success();
                return;
            }
            Err(error) => {
                respawn_state.record_failure(&error);
                emit_inscription(
                    "relay.acp.respawn.attempt_failed",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempt": respawn_state.attempt,
                        "error_code": error.code,
                        "reason": error.reason,
                    }),
                );
                if error.is_permanent() || respawn_state.should_give_up() {
                    transport
                        .lock()
                        .expect("acp transport mutex poisoned")
                        .mark_runtime_unavailable();
                    (services.record_failure)(WorkerFailureReason {
                        code: error.code.clone(),
                        reason: error.reason.clone(),
                    });
                    (services.mirror_state)(WorkerReadinessState::Unavailable);
                    emit_inscription(
                        "relay.acp.respawn.permanent_failure",
                        &json!({
                            "namespace": namespace,
                            "target_session": target_session,
                            "attempts": respawn_state.attempt,
                            "final_error_code": error.code,
                            "reason": error.reason,
                        }),
                    );
                    (services.broadcast_ui)(
                        "acp_worker_respawn_completed",
                        json!({
                            "attempts": respawn_state.attempt,
                            "outcome": "permanent_failure",
                            "final_error_code": error.code,
                            "reason": error.reason,
                        }),
                    );
                    return;
                }
            }
        }
    }
}

async fn sleep_with_shutdown_gate(duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        if shutdown_requested() {
            return false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let poll = remaining.min(Duration::from_millis(RESPAWN_SLEEP_POLL_MS));
        if poll.is_zero() {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    !shutdown_requested()
}

fn respawn_backoff_cap_ms() -> u64 {
    std::env::var(RESPAWN_BACKOFF_MAX_MS_ENVVAR)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(RESPAWN_BACKOFF_CAP_DEFAULT_MS)
}

struct AcpRespawnState {
    attempt: u32,
    next_backoff_ms: u64,
    last_failure_signature: Option<(String, String)>,
    consecutive_identical_failures: u32,
}

impl AcpRespawnState {
    fn new() -> Self {
        Self {
            attempt: 0,
            next_backoff_ms: 0,
            last_failure_signature: None,
            consecutive_identical_failures: 0,
        }
    }

    fn advance(&mut self) -> Duration {
        let cap = respawn_backoff_cap_ms();
        let backoff = if self.next_backoff_ms == 0 {
            RESPAWN_BACKOFF_INITIAL_MS.min(cap)
        } else {
            self.next_backoff_ms.min(cap)
        };
        self.next_backoff_ms = backoff.saturating_mul(2).min(cap);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_millis(backoff)
    }

    fn record_failure(&mut self, error: &AcpBootstrapError) {
        let signature = (error.code.clone(), error.reason.clone());
        if self.last_failure_signature.as_ref() == Some(&signature) {
            self.consecutive_identical_failures =
                self.consecutive_identical_failures.saturating_add(1);
        } else {
            self.last_failure_signature = Some(signature);
            self.consecutive_identical_failures = 1;
        }
    }

    fn should_give_up(&self) -> bool {
        self.consecutive_identical_failures >= RESPAWN_REPEATED_FAILURE_THRESHOLD
    }

    fn reset_on_success(&mut self) {
        self.attempt = 0;
        self.next_backoff_ms = 0;
        self.last_failure_signature = None;
        self.consecutive_identical_failures = 0;
    }
}
