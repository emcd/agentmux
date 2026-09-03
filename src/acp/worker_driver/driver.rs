use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::configuration::BundleMember;
use crate::envelope::PromptBatchSettings;
use crate::transports::{
    GenerationFence, OutputView, StartupContext, Transport, TransportError, TransportHealth,
    TransportStatus, UnreachableSince, WorkerReadinessState,
};

use crate::acp::AcpTransport;

use super::respawn::{
    AbandonmentSignal, AcpRespawnState, AcpRespawnTarget, acp_respawn_monitor,
    initial_acp_bootstrap,
};
use super::services::AcpDriverServices;

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
        delivery: crate::transports::DeliveryExecutorContext,
    ) -> Self {
        // Built before the transport, because the transport hands them to its
        // executor: under the pull model the executor is the only thing that
        // observes health, so the latch the driver folds and the latch the dwell
        // is measured from have to be the same one.
        let respawn_abandoned = Arc::new(AtomicBool::new(false));
        let unreachable_since = Arc::new(UnreachableSince::default());
        Self {
            transport: Arc::new(Mutex::new(AcpTransport::new(
                batch_settings,
                Some(Arc::clone(&services.mirror_state)),
                delivery,
                crate::acp::AcpReachability::new(
                    Arc::clone(&respawn_abandoned),
                    Arc::clone(&unreachable_since),
                ),
            ))),
            namespace,
            runtime_directory,
            target_member,
            services,
            respawn_monitor: None,
            bootstrap_task: None,
            respawn_abandoned,
            unreachable_since,
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
