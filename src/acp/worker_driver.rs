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
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::configuration::BundleMember;
use crate::envelope::PromptBatchSettings;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::signals::shutdown_requested;
use crate::transports::contract::OutcomeFuture;
use crate::transports::{
    Chooser, DeliveryEnvelope, GenerationFence, OutputView, PartitionSink, StartupContext,
    Transport, TransportError, TransportHealth, TransportStatus, UnreachableSince,
    WorkerFailureReason, WorkerReadinessState,
};

use super::{AcpBootstrapError, AcpTransport, bootstrap_acp_worker_runtime};

const RESPAWN_BACKOFF_MAX_MS_ENVVAR: &str = "AGENTMUX_RELAY_ACP_RESPAWN_BACKOFF_MAX_MS";
const RESPAWN_SLEEP_POLL_MS: u64 = 50;
const RESPAWN_BACKOFF_INITIAL_MS: u64 = 1_000;
const RESPAWN_BACKOFF_CAP_DEFAULT_MS: u64 = 30_000;
/// Attempts one worker may consume before respawn gives up, counted across every
/// trigger rather than per burst and cleared only by a successful re-establish.
///
/// Deliberately blind to *which* failure occurred. The predecessor counted
/// consecutive **identical** failure signatures, which stopped fast on a
/// deterministic fault and never stopped at all on an alternating one — a worker
/// failing A, B, A, B reset the counter every time and retried forever. Stopping
/// sooner on the deterministic case was a latency optimisation; failing to stop
/// on the alternating case was the bug.
///
/// This bound matters more than it used to. Recovery used to be driven by
/// delivery attempts, so a hopeless target stopped being retried once senders
/// gave up on it — traffic was an accidental circuit breaker. The respawn signal
/// now persists while the runtime stays `Unavailable`, so the monitor retries on
/// its own clock and nothing else bounds the loop.
const RESPAWN_ATTEMPT_LIMIT: u32 = 6;
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
    /// The relay's guard, for reporting which members share one `session/prompt`.
    /// Handed to the transport at construction; see
    /// [`PartitionSink`](crate::transports::PartitionSink).
    pub partition_sink: Arc<dyn PartitionSink>,
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
/// driver-owned async respawn monitor (spawned in
/// [`start_bootstrap`](Self::start_bootstrap))
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
    /// The driver-owned respawn monitor.
    ///
    /// Retained rather than detached: the monitor can install a replacement
    /// runtime, so it is a generation-owned executor like any other, and one
    /// whose handle is discarded can be neither terminated nor observed. Absent
    /// until bootstrap spawns it.
    respawn_monitor: Option<tokio::task::JoinHandle<()>>,
    /// The driver-owned initial-bootstrap task.
    ///
    /// The initial bootstrap runs as a supervised task rather than inline in the
    /// relay worker's startup, because a worker awaiting it cannot reach its own
    /// shutdown gate: an agent parked in `initialize` meant the worker never
    /// entered its loop, never began a fence, and never emitted a verdict at all
    /// — not even the honest negative one. Retained for the same reason the
    /// respawn monitor is: it spawns and owns an agent child, so it is a
    /// generation-owned executor, and one whose handle is discarded can be
    /// neither terminated nor observed.
    bootstrap_task: Option<tokio::task::JoinHandle<()>>,
    /// Set once respawn has given up on this target, either because the failure
    /// was permanent or because the retry budget ran out.
    ///
    /// This, and not `WorkerReadinessState::Unavailable`, is what the health
    /// axis reads. `Unavailable` is published for a respawn gap as readily as
    /// for a give-up, so reading it would report a worker whose replacement is
    /// seconds away as unreachable and bounce the members that replacement was
    /// about to serve.
    ///
    /// Shared with the respawn monitor, which is the task that discovers the
    /// give-up and outlives no driver.
    respawn_abandoned: Arc<AtomicBool>,
    /// Latch for the health axis; see [`Transport::health`].
    ///
    /// Shared with the respawn monitor so the instant is recorded at the
    /// give-up transition rather than at the first later `health()` call. The
    /// dwell measures how long the target has been unreachable, not how long
    /// since someone happened to ask: a member arriving well after a permanent
    /// failure would otherwise wait a fresh full dwell for a target that was
    /// already known to be gone.
    unreachable_since: Arc<UnreachableSince>,
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
                Arc::clone(&services.partition_sink),
            ))),
            namespace,
            runtime_directory,
            target_member,
            services,
            respawn_monitor: None,
            bootstrap_task: None,
            respawn_abandoned: Arc::new(AtomicBool::new(false)),
            unreachable_since: Arc::new(UnreachableSince::default()),
        }
    }

    /// Locks the shared transport. Locks are brief by construction (the respawn
    /// monitor never holds the lock across `.await` or the blocking child spawn).
    fn lock_transport(&self) -> std::sync::MutexGuard<'_, AcpTransport> {
        self.transport.lock().expect("acp transport mutex poisoned")
    }

    /// Starts establishing the ACP runtime and returns the flag the relay worker
    /// gates task admission on.
    ///
    /// Everything that must be true before the worker's loop runs happens here,
    /// synchronously: readiness reads Initializing, the look handle is published
    /// (so a `look` in the initial-startup window finds it), and the
    /// chooser/target identity is set. Only the part that can block for an
    /// unbounded time — spawning the agent and completing the ACP handshake —
    /// is handed to a supervised task.
    ///
    /// The returned flag reads `true` once that task has settled, whichever way
    /// it settled. The worker holds its queue until then, which preserves the
    /// delivery semantics of the awaited bootstrap this replaces: no submission
    /// reaches a transport that has no runtime yet. What changes is that the
    /// waiting now happens inside a loop whose shutdown gate stays reachable, so
    /// a bootstrap that never returns still reaches the fence.
    pub fn start_bootstrap(&mut self) -> Arc<AtomicBool> {
        (self.services.mirror_state)(WorkerReadinessState::Initializing);
        let handle = self.lock_transport().give_output();
        (self.services.publish_output)(handle);

        // Set chooser/target identity ahead of the establish; the freshly-built
        // transport already reads Initializing from construction.
        self.lock_transport()
            .prepare_for_startup(self.services.chooser.clone(), self.target_member.id.clone());

        // Spawned before the bootstrap it supervises, rather than after it
        // returns: the monitor is idle until the transport signals
        // respawn-needed, and a bootstrap that never returns must not be the
        // reason the target has no supervisor.
        self.spawn_respawn_monitor();

        let settled = Arc::new(AtomicBool::new(false));
        self.bootstrap_task = Some(tokio::spawn(initial_acp_bootstrap(
            Arc::clone(&self.transport),
            self.services.clone(),
            self.namespace.clone(),
            self.runtime_directory.clone(),
            self.target_member.clone(),
            AbandonmentSignal {
                abandoned: Arc::clone(&self.respawn_abandoned),
                unreachable_since: Arc::clone(&self.unreachable_since),
            },
            Arc::clone(&settled),
        )));
        settled
    }

    /// Spawns the driver-owned async respawn monitor. It subscribes to the
    /// transport's stable respawn-needed signal and drives respawn off the relay
    /// worker loop, sharing the transport via `Arc<Mutex<…>>`.
    fn spawn_respawn_monitor(&mut self) {
        let transport = Arc::clone(&self.transport);
        let respawn_needed = self.lock_transport().respawn_needed_subscribe();
        let services = self.services.clone();
        let namespace = self.namespace.clone();
        let runtime_directory = self.runtime_directory.clone();
        let target_member = self.target_member.clone();
        self.respawn_monitor = Some(tokio::spawn(acp_respawn_monitor(
            transport,
            respawn_needed,
            services,
            AcpRespawnState::new(),
            AcpRespawnTarget {
                namespace,
                runtime_directory,
                member: target_member,
            },
            Arc::clone(&self.respawn_abandoned),
            Arc::clone(&self.unreachable_since),
        )));
    }
}

impl GenerationFence for AcpWorkerDriver {
    fn fence_generation(&mut self) {
        // Marking the transport fenced is also the monitor's cooperative stop
        // request: it reads the same flag between polls and returns.
        self.lock_transport().fence_generation();
    }

    fn terminate_generation(&mut self) {
        self.lock_transport().terminate_generation();
        // Either task can be parked in a bootstrap that will not return inside
        // any window the fence allows. Aborting is the destructive step for a
        // task that observes nothing; the runtime such a bootstrap would have
        // produced is refused at the install anyway, under the fenced check, and
        // is torn down by the closure that owns it rather than left to whichever
        // task was awaiting the result.
        if let Some(monitor) = self.respawn_monitor.as_ref() {
            monitor.abort();
        }
        if let Some(bootstrap) = self.bootstrap_task.as_ref() {
            bootstrap.abort();
        }
    }

    fn generation_ceased(&self) -> bool {
        let monitor_ceased = self
            .respawn_monitor
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished);
        let bootstrap_ceased = self
            .bootstrap_task
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished);
        monitor_ceased && bootstrap_ceased && self.lock_transport().generation_ceased()
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

    async fn is_ready_for_handover(&self) -> bool {
        // Read readiness without holding the lock across an await. Delegates
        // to the transport's sync predicate so the four ACP readiness call
        // sites stay consistent.
        self.lock_transport().is_available()
    }

    fn health(&self) -> TransportHealth {
        // Reachability for an ACP target means a replacement is still possible,
        // not that one exists right now. A respawn gap reports `Unavailable` and
        // is healthy: the monitor is mid-flight and the member it would bounce is
        // the member the replacement is about to serve. Only an abandoned respawn
        // — permanent failure or an exhausted retry budget — is unreachable.
        self.unreachable_since
            .fold(!self.respawn_abandoned.load(Ordering::Acquire))
    }

    fn shutdown(&mut self) {
        self.lock_transport().shutdown();
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        self.lock_transport().give_output()
    }
}

/// What one bootstrap attempt did with the runtime it produced.
enum BootstrapDisposition {
    /// The runtime is live and installed on the transport.
    Installed,
    /// A fence began while the bootstrap was running, so the runtime was refused
    /// and shut down inside the bootstrap closure.
    RefusedAfterFence,
    /// No runtime was produced.
    Failed(AcpBootstrapError),
}

/// Runs one bootstrap on the blocking pool, disposes of the runtime it produces
/// — installing it, or tearing it down when a fence refuses it — and publishes
/// the readiness that disposal leaves behind, all inside that same closure.
///
/// None of that can be left to the task awaiting the result. A bootstrap runs on
/// a blocking pool, and `tokio`'s abort cancels only the awaiting task, never
/// the closure; a bootstrap that finishes after such an abort would hand a live
/// agent child to nobody and would leave the target advertising a runtime that
/// is still on its way. So the split is by what the work belongs to rather than
/// by what is convenient: everything that must happen no matter who is still
/// listening lives here, and only the retry policy — backoff state, per-attempt
/// inscriptions — stays with the caller. Owning creation through
/// install-or-teardown here is also what makes the in-flight count mean
/// something: it spans the runtime's whole exposure rather than stopping at the
/// moment it was constructed, when nothing had yet decided the child's fate.
///
/// A failed attempt publishes nothing, because whether it is terminal is exactly
/// the retry policy's question.
///
/// The transport lock is taken only for the install decision, which is a few
/// field updates and a thread spawn — never across the child spawn or the ACP
/// handshake, so a concurrent `mailw` is not stalled by a bootstrap.
async fn run_one_bootstrap(
    transport: &Arc<Mutex<AcpTransport>>,
    mirror_state: &MirrorStateFn,
    runtime_directory: PathBuf,
    target_member: BundleMember,
) -> BootstrapDisposition {
    let in_flight = transport
        .lock()
        .expect("acp transport mutex poisoned")
        .begin_bootstrap();
    let transport = Arc::clone(transport);
    let mirror_state = Arc::clone(mirror_state);
    tokio::task::spawn_blocking(move || {
        let publish = |generation| in_flight.publish_generation(generation);
        match bootstrap_acp_worker_runtime(&runtime_directory, &target_member, &publish) {
            Ok(runtime) => {
                // The check and the install happen under one lock: a fence that
                // began while this bootstrap ran must not find a fresh agent
                // installed into a generation it has already declared stopped.
                let refused = transport
                    .lock()
                    .expect("acp transport mutex poisoned")
                    .install_runtime_unless_fenced(runtime);
                match refused {
                    Some(mut refused) => {
                        refused.client.shutdown();
                        // The target has no runtime and a fenced generation
                        // admits no replacement, so Unavailable is the accurate
                        // terminal state. Leaving it Initializing or Recovering
                        // told every observer — the TUI, the startup poller, a
                        // `list` — that a runtime was still on its way.
                        mirror_state(WorkerReadinessState::Unavailable);
                        BootstrapDisposition::RefusedAfterFence
                    }
                    None => {
                        mirror_state(WorkerReadinessState::Available);
                        BootstrapDisposition::Installed
                    }
                }
            }
            Err(error) => BootstrapDisposition::Failed(error),
        }
    })
    .await
    .expect("ACP bootstrap task panicked")
}

/// The driver-owned initial bootstrap: establishes the first runtime for a
/// target and reports the outcome through the relay touchpoints.
///
/// Runs as its own task so the relay worker's loop — and therefore its shutdown
/// gate — is reachable throughout. `settled` is raised on every path that
/// returns, so the worker knows when its queue may start flowing; a bootstrap
/// aborted by a fence never raises it, which is correct, because that worker is
/// draining rather than delivering.
async fn initial_acp_bootstrap(
    transport: Arc<Mutex<AcpTransport>>,
    services: AcpDriverServices,
    namespace: String,
    runtime_directory: PathBuf,
    target_member: BundleMember,
    abandonment: AbandonmentSignal,
    settled: Arc<AtomicBool>,
) {
    let target_session = target_member.id.clone();
    let disposition = run_one_bootstrap(
        &transport,
        &services.mirror_state,
        runtime_directory,
        target_member,
    )
    .await;
    match disposition {
        BootstrapDisposition::Installed => {}
        BootstrapDisposition::RefusedAfterFence => {
            emit_inscription(
                "relay.acp.worker.bootstrap_refused_after_fence",
                &json!({
                    "namespace": namespace,
                    "target_session": target_session,
                }),
            );
        }
        BootstrapDisposition::Failed(error) => {
            transport
                .lock()
                .expect("acp transport mutex poisoned")
                .mark_runtime_unavailable();
            // Record the failure before the readiness transition so the startup
            // poller, which acts the moment it observes Unavailable, finds the
            // true cause already stored.
            (services.record_failure)(WorkerFailureReason {
                code: error.code.clone(),
                reason: error.reason.clone(),
            });
            (services.mirror_state)(WorkerReadinessState::Unavailable);
            emit_inscription(
                "relay.acp.worker.bootstrap_failed",
                &json!({
                    "namespace": namespace,
                    "target_session": target_session,
                    "error_code": error.code,
                    "reason": error.reason,
                }),
            );
            if error.is_permanent() {
                // A permanent bootstrap failure gets no respawn signal: the
                // monitor would only run one more attempt to discover the same
                // permanence and abandon. Latch abandonment here instead, so the
                // health axis reads unreachable immediately and the dwell clock
                // starts at the transition rather than at a later enquiry.
                abandonment.abandoned.store(true, Ordering::Release);
                let _ = abandonment.unreachable_since.fold(false);
                emit_inscription(
                    "relay.acp.respawn.permanent_failure",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempts": 1,
                        "final_error_code": error.code,
                        "reason": error.reason,
                    }),
                );
                (services.broadcast_ui)(
                    "acp_worker_respawn_completed",
                    json!({
                        "attempts": 1,
                        "outcome": "permanent_failure",
                        "final_error_code": error.code,
                        "reason": error.reason,
                    }),
                );
            } else {
                // No delivery task is running to emit the respawn-needed signal,
                // so prime it directly: the monitor will retry with backoff.
                transport
                    .lock()
                    .expect("acp transport mutex poisoned")
                    .signal_respawn();
            }
        }
    }
    settled.store(true, Ordering::Release);
}

/// Driver-owned async respawn monitor. Subscribes to the transport's stable
/// respawn-needed signal and drives respawn off the relay worker loop. The
/// transport is shared via `Arc<Mutex<AcpTransport>>`; the monitor locks only for
/// the fast release/install steps — never across `.await` or the blocking child
/// spawn — so a concurrent worker `mailw` is never stalled. Exits on relay
/// shutdown.
/// The target one respawn monitor supervises.
///
/// Grouped rather than passed as three parallel parameters: they are one
/// cohesive identity — which target, where its runtime lives, and how it is
/// configured — and every re-establish attempt needs all three together.
/// Whether the monitor owes this target a respawn attempt.
///
/// Extracted from the monitor loop because the interesting cases are
/// *combinations* — a signal without an `Unavailable` runtime, an `Unavailable`
/// runtime without a signal — and staging those against a live monitor means
/// racing a respawn to observe a state it is about to leave.
fn respawn_is_owed(signalled: bool, abandoned: bool, readiness: WorkerReadinessState) -> bool {
    signalled && !abandoned && matches!(readiness, WorkerReadinessState::Unavailable)
}

/// The health signals a respawn (or a permanent initial-bootstrap failure)
/// writes when it gives up on a target.
///
/// Paired rather than passed separately because they are one fact recorded in
/// two places — that the target is past recovery, and when that became true —
/// and separating them is exactly how the instant came to be stamped at first
/// enquiry instead of at the transition. Owned so the pair can be handed to the
/// spawned bootstrap task alongside the respawn monitor.
struct AbandonmentSignal {
    abandoned: Arc<AtomicBool>,
    unreachable_since: Arc<UnreachableSince>,
}

struct AcpRespawnTarget {
    namespace: String,
    runtime_directory: PathBuf,
    member: BundleMember,
}

async fn acp_respawn_monitor(
    transport: Arc<Mutex<AcpTransport>>,
    mut respawn_needed: tokio::sync::watch::Receiver<u64>,
    services: AcpDriverServices,
    mut respawn_state: AcpRespawnState,
    target: AcpRespawnTarget,
    respawn_abandoned: Arc<AtomicBool>,
    unreachable_since: Arc<UnreachableSince>,
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
        // The fence's cooperative request. A fenced generation admits no
        // replacement, so there is nothing left for this monitor to do and
        // continuing would only race the install check.
        if transport
            .lock()
            .expect("acp transport mutex poisoned")
            .generation_is_fenced()
        {
            return;
        }
        // Three conditions, each excluding something the others cannot.
        //
        // The **signal** carries the classification. Not every `Unavailable` is
        // a respawn: the delivery task raises the signal only on a connection
        // close or transport write failure, and deliberately not on a
        // non-delivery like serialization failure, which is not recoverable by
        // restarting the agent. A level-only trigger would respawn on it and
        // override that judgement.
        //
        // The **level** guards against staleness. A producer's edge can arrive
        // after the runtime it described has already been replaced, and acting
        // on it would tear down a healthy generation to recover from a failure
        // that is already over.
        //
        // **Abandonment** guards the crash loop that `RESPAWN_ATTEMPT_LIMIT`
        // exists to stop.
        //
        // What makes this independent of delivery traffic is not a level-only
        // trigger but the signal's *persistence*: it is cleared below only once
        // the runtime is no longer `Unavailable`, so a failed attempt leaves it
        // standing and the monitor retries on its own clock. Re-priming from
        // `mailw` — recovery only because something tried to write — is what
        // that replaces.
        respawn_needed.borrow_and_update();
        let abandoned = respawn_abandoned.load(Ordering::Acquire);
        let (outstanding, readiness) = {
            let transport = transport.lock().expect("acp transport mutex poisoned");
            (
                transport.respawn_signal_outstanding(),
                transport.readiness(),
            )
        };
        if !respawn_is_owed(outstanding.is_some(), abandoned, readiness) {
            // Once raised, "owed" and "answered" are exact complements: owed is
            // `!abandoned && Unavailable`, so an outstanding cause that is not
            // owed has necessarily been answered -- the runtime left
            // `Unavailable` under its own power, or abandonment closed the
            // account.
            //
            // Retiring it here and not only after this monitor's own attempt is
            // what keeps a cause from outliving the failure it described. One
            // left standing across a recovery stays outstanding, and the next
            // `Unavailable` -- including a serialization failure, which the
            // delivery task deliberately does not signal -- would find all
            // three conditions met and respawn on a classification that was
            // never made for it. The level check cannot catch that on its own:
            // it distinguishes states, not which failure put the runtime in
            // one.
            //
            // Retiring *this* epoch rather than clearing the signal is what
            // makes the decision safe to act on late. Everything above is a
            // sample: the lock is released before the retirement lands, so a
            // live failure can publish a new cause in between. That cause
            // carries a higher epoch, so the retirement bounds only what was
            // classified and leaves the new one outstanding for the next tick.
            if let Some(epoch) = outstanding {
                transport
                    .lock()
                    .expect("acp transport mutex poisoned")
                    .retire_respawn_signal(epoch);
            }
            continue;
        }
        run_acp_respawn(
            &transport,
            &services,
            &mut respawn_state,
            target.namespace.as_str(),
            target.runtime_directory.as_path(),
            &target.member,
            AbandonmentSignal {
                abandoned: Arc::clone(&respawn_abandoned),
                unreachable_since: Arc::clone(&unreachable_since),
            },
        )
        .await;
        // Retire the cause only once it has been answered — the runtime is no
        // longer `Unavailable`, or respawn has been abandoned and no further
        // attempt is coming. Retiring unconditionally is what made an external
        // re-prime necessary: an attempt that failed without exhausting the
        // budget left the worker dead with nothing left to say so, and recovery
        // then waited for a write that the readiness gate would never allow.
        //
        // Holding the cause while `Unavailable` persists also keeps the
        // classification intact across retries. It was raised because *this*
        // failure warranted a respawn; that judgement does not expire because an
        // attempt did not take.
        //
        // The epoch retired is the one this attempt answered, never whatever is
        // current. A respawn runs for a while, and a failure on the way back up
        // can publish a cause of its own; bounding by the classified epoch
        // leaves that one for the next tick instead of consuming it here.
        let answered = {
            let transport = transport.lock().expect("acp transport mutex poisoned");
            !matches!(transport.readiness(), WorkerReadinessState::Unavailable)
        } || respawn_abandoned.load(Ordering::Acquire);
        if answered {
            let epoch = outstanding.expect("an owed respawn has an outstanding cause");
            transport
                .lock()
                .expect("acp transport mutex poisoned")
                .retire_respawn_signal(epoch);
        }
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
    abandonment: AbandonmentSignal,
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
            .prepare_for_startup(services.chooser.clone(), target_member.id.clone());

        // Bootstrap the new runtime OFF the lock (blocking child spawn). The
        // install happens inside that same closure, so the published handle
        // stays valid (install repoints its replay slot) and a fence that began
        // mid-bootstrap refuses the runtime and tears it down there.
        let disposition = run_one_bootstrap(
            transport,
            &services.mirror_state,
            runtime_directory.to_path_buf(),
            target_member.clone(),
        )
        .await;

        match disposition {
            BootstrapDisposition::RefusedAfterFence => {
                emit_inscription(
                    "relay.acp.respawn.refused_after_fence",
                    &json!({
                        "namespace": namespace,
                        "target_session": target_session,
                        "attempt": respawn_state.attempt,
                    }),
                );
                return;
            }
            BootstrapDisposition::Installed => {
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
            BootstrapDisposition::Failed(error) => {
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
                    // Latch the health axis here, at the one place that knows no
                    // further attempt is coming. Every other route to
                    // `Unavailable` is survivable, so this is the only signal
                    // that separates a target worth waiting for from one that
                    // will never come back.
                    // Both halves of the same fact, recorded together: that this
                    // target is past recovery, and when that became true. Stamping
                    // the instant here rather than leaving it to whenever a member
                    // next asks is what keeps the dwell measuring how long the
                    // target has been unreachable — starting the clock on first
                    // enquiry would charge a late arrival a full fresh dwell for a
                    // target already known to be gone.
                    abandonment.abandoned.store(true, Ordering::Release);
                    let _ = abandonment.unreachable_since.fold(false);
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
}

impl AcpRespawnState {
    fn new() -> Self {
        Self {
            attempt: 0,
            next_backoff_ms: 0,
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

    fn should_give_up(&self) -> bool {
        self.attempt >= RESPAWN_ATTEMPT_LIMIT
    }

    fn reset_on_success(&mut self) {
        self.attempt = 0;
        self.next_backoff_ms = 0;
    }
}

#[cfg(test)]
mod respawn_owed_tests {
    use super::{WorkerReadinessState, respawn_is_owed};

    /// The three conditions each exclude something the others cannot, so the
    /// matrix is the assertion.
    ///
    /// Crate-private by design: the owed condition is monitor internals with no
    /// public consumer, and widening it to reach from `tests/unit` would add API
    /// surface that exists only for this check.
    #[test]
    fn a_respawn_is_owed_only_when_signal_level_and_budget_agree() {
        // The positive case: a failure that warranted a respawn, a runtime still
        // dead, and a budget that has not run out.
        assert!(respawn_is_owed(
            true,
            false,
            WorkerReadinessState::Unavailable
        ));

        // A stale edge. A producer's signal can arrive after the runtime it
        // described has already been replaced; acting on it would tear down a
        // healthy generation to recover from a failure that is already over. The
        // level is what makes the edge safe to receive late.
        for recovered in [
            WorkerReadinessState::Available,
            WorkerReadinessState::Busy,
            WorkerReadinessState::Recovering,
            WorkerReadinessState::Initializing,
        ] {
            assert!(
                !respawn_is_owed(true, false, recovered),
                "a signal must not respawn a runtime that is no longer Unavailable: {recovered:?}"
            );
        }

        // An Unavailable runtime with no signal. Not every Unavailable warrants a
        // respawn -- the delivery task raises the signal only on a connection
        // close or transport write failure, and never for a non-delivery like
        // serialization failure, which is not fixed by restarting the agent. The
        // signal is where that judgement lives, so a level-only trigger would
        // override it.
        assert!(
            !respawn_is_owed(false, false, WorkerReadinessState::Unavailable),
            "an Unavailable the transport did not signal for is not a respawn"
        );

        // Abandoned: past recovery whatever else is true. This is the crash loop
        // `RESPAWN_ATTEMPT_LIMIT` exists to stop.
        assert!(!respawn_is_owed(
            true,
            true,
            WorkerReadinessState::Unavailable
        ));
    }
}
