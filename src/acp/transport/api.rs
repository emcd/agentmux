//! AcpTransport core — lifecycle, readiness, and the public Transport surface.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::acp::client::{AcpGenerationHandle, SharedReplay};
use crate::acp::persistent_runtime::PersistentAcpWorkerRuntime;
use crate::envelope::PromptBatchSettings;
use crate::transports::WorkerReadinessState;
use crate::transports::{
    DeliveryExecutorContext, GenerationFence, OutputView, StartupContext, Transport,
    TransportError, TransportHealth, TransportStatus, run_delivery_executor,
};

use super::delivery::{
    AcpDeliveryWriter, AcpReachability, AcpRuntimeSlot, DeliveryChannels, DeliveryTaskIdentity,
    RuntimeInstall,
};
use super::state::{
    AcpSharedState, BootstrapInFlight, BootstrapRecord, BootstrapRegistry, NEXT_BOOTSTRAP_ID,
    ReadinessMirror, raise_respawn_signal,
};

pub struct AcpTransport {
    runtime: Option<PersistentAcpWorkerRuntime>,
    chooser: Option<crate::transports::Chooser>,
    shared: Arc<AcpSharedState>,
    /// What the relay injected so this transport's delivery-loop executor can
    /// reach its target's mailbox. Held rather than consumed because the executor
    /// is spawned once and outlives every runtime this transport establishes.
    delivery: DeliveryExecutorContext,
    /// Hands established runtimes to the executor, and tells it when one is
    /// released. `None` before the executor is spawned.
    runtime_tx: Option<std::sync::mpsc::Sender<RuntimeInstall>>,
    /// Stop request for the delivery executor, latched by `fence_generation` and
    /// `shutdown`. A flag rather than a dropped channel because the executor now
    /// outlives the runtimes: a signal tied to one of them would end the executor
    /// at the first respawn.
    executor_stop: Arc<AtomicBool>,
    /// What the driver knows about permanence, shared into the executor so the
    /// dwell is carried by the one thing that observes health under the pull
    /// model.
    reachability: AcpReachability,
    /// Prompt-batch settings (token budget and tokenizer profile) for envelope
    /// combining.
    batch_settings: PromptBatchSettings,
    /// Target session id, captured at startup for permission correlation.
    target_session: String,
    /// Stable respawn-needed signal, as a monotonically increasing epoch.
    ///
    /// Created once at construction (not per delivery task) so the driver-owned
    /// async respawn monitor can hold a single long-lived subscription across
    /// respawns. Driven exclusively from the transport (bootstrap failure,
    /// write-without-runtime paths); readiness latches no longer promote
    /// themselves into respawn signals.
    ///
    /// An epoch rather than a flag because a flag cannot name *which* failure
    /// asked for the respawn, and every consumer of this signal needs that. A
    /// bare `bool` makes classification and retirement two operations on the
    /// same indistinguishable value: the monitor decides an outstanding signal
    /// has been answered, and by the time it writes that decision down, the
    /// value may describe a different, live failure that the write would erase.
    /// The counter never resets, so an epoch identifies its cause for the life
    /// of the transport and a decision made about one can never land on
    /// another. Retirement is a high-water mark
    /// ([`respawn_retired`](Self::respawn_retired)), not a reset.
    respawn_needed_tx: tokio::sync::watch::Sender<u64>,
    /// Highest respawn epoch the monitor has finished with.
    ///
    /// The signal is outstanding while the raised epoch exceeds this. Retiring
    /// records a bound rather than clearing a flag, which is what makes
    /// retirement safe against a concurrent raise: a cause published after the
    /// monitor sampled its epoch carries a strictly greater one, so it stays
    /// outstanding no matter when the retirement lands.
    respawn_retired: AtomicU64,
    /// `JoinHandle` for this transport's one delivery executor. Retained so a
    /// generation supervisor can observe its cessation (the binding the fence
    /// requires) and detach it cleanly on `take`. Set once, by `ensure_executor`,
    /// and never replaced: a respawn installs a runtime into the running
    /// executor, so there is no second thread for a second handle to name.
    delivery_task_handle: Option<JoinHandle<()>>,
    /// Fencing surface of the generation whose client the delivery task owns.
    ///
    /// Taken from the client at the moment it is moved into that task, because
    /// `self.runtime` is emptied by the same move: reading termination and
    /// cessation off the runtime made step 3 a no-op and made the reader
    /// observation vacuously true, since both looked at a field that is `None`
    /// for the whole steady state.
    generation: Option<AcpGenerationHandle>,
    /// Whether this generation has been fenced.
    ///
    /// One-way, and read by everything that could otherwise install a
    /// replacement runtime behind the fence's back. The respawn monitor runs
    /// asynchronously and can be mid-bootstrap when a fence begins, so the
    /// decision to install has to be taken against this flag under the same lock
    /// as the install itself.
    fenced: Arc<AtomicBool>,
    /// The bootstraps this generation currently has running, and the agent child
    /// each one owns.
    ///
    /// A bootstrap spawns and owns an agent child, so it is a generation-owned
    /// executor — but it runs on a blocking pool, and aborting the async task
    /// awaiting it does not cancel the closure. Observing the wrapper therefore
    /// says nothing about whether the executor stopped, and it offers the fence
    /// nothing to signal either; the child is what both steps have to reach.
    ///
    /// A list rather than a single slot, for the same reason this was a count
    /// before it carried handles: an initial bootstrap and a respawn can overlap,
    /// and neither may clear or terminate the other's state.
    bootstraps: Arc<BootstrapRegistry>,
}

impl std::fmt::Debug for AcpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpTransport")
            .field("has_runtime", &self.runtime.is_some())
            .field("readiness", &self.readiness())
            .field("executor_running", &self.delivery_task_handle.is_some())
            .field("batch_settings", &self.batch_settings)
            .finish()
    }
}

impl AcpTransport {
    #[must_use]
    pub fn new(
        batch_settings: PromptBatchSettings,
        mirror_state: Option<ReadinessMirror>,
        delivery: DeliveryExecutorContext,
        reachability: AcpReachability,
    ) -> Self {
        Self {
            runtime: None,
            chooser: None,
            shared: Arc::new(AcpSharedState {
                readiness: Mutex::new(WorkerReadinessState::Initializing),
                replay: Mutex::new(None),
                mirror_state,
                permission_executors: Mutex::new(Vec::new()),
            }),
            delivery,
            runtime_tx: None,
            executor_stop: Arc::new(AtomicBool::new(false)),
            reachability,
            batch_settings,
            target_session: String::new(),
            respawn_needed_tx: tokio::sync::watch::channel(0).0,
            respawn_retired: AtomicU64::new(0),
            delivery_task_handle: None,
            generation: None,
            fenced: Arc::new(AtomicBool::new(false)),
            bootstraps: Arc::new(BootstrapRegistry::default()),
        }
    }

    /// Subscribes to the stable respawn-needed signal. The driver-owned respawn
    /// monitor holds one subscription for the transport's whole life.
    pub fn respawn_needed_subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.respawn_needed_tx.subscribe()
    }

    /// The outstanding respawn cause's epoch, if any is still owed an answer.
    #[must_use]
    pub fn respawn_signal_outstanding(&self) -> Option<u64> {
        let raised = *self.respawn_needed_tx.borrow();
        (raised > self.respawn_retired.load(Ordering::Acquire)).then_some(raised)
    }

    /// Marks every respawn cause up to and including `epoch` as answered, and
    /// reports whether any signal remains outstanding afterwards.
    ///
    /// Retirement is a high-water mark rather than a reset, which is the whole
    /// point: a caller retires the epoch it *classified*, and a cause published
    /// between that classification and this call carries a strictly greater
    /// epoch. `fetch_max` therefore cannot erase it — the newer cause is still
    /// outstanding when this returns, and the caller learns so from the return
    /// value. Resetting a flag instead loses that cause silently, and because
    /// the readiness gate withholds the very writes that would raise it again,
    /// losing one is not a delayed recovery but a permanent one.
    ///
    /// `fetch_max` also makes the operation idempotent and order-independent,
    /// so a retirement that arrives late can never move the mark backwards.
    pub fn retire_respawn_signal(&self, epoch: u64) -> Option<u64> {
        self.respawn_retired.fetch_max(epoch, Ordering::AcqRel);
        self.respawn_signal_outstanding()
    }

    /// Publishes a fresh respawn cause. Used after an initial-bootstrap failure
    /// so the driver's respawn monitor retries with backoff, and by the delivery
    /// paths whose outcome warrants replacing the runtime.
    pub fn signal_respawn(&self) {
        raise_respawn_signal(&self.respawn_needed_tx);
    }

    /// Takes and clears the most recent delivery task's [`JoinHandle`].
    /// A generation supervisor binds the handle so it can join the thread
    /// and observe cessation as part of fence acknowledgment. Returns `None`
    /// when the transport has never spawned a delivery task or the handle
    /// has already been taken for the current generation.
    pub fn take_delivery_task_handle(&mut self) -> Option<JoinHandle<()>> {
        self.delivery_task_handle.take()
    }

    /// Current readiness, mirrored by the `AcpWorkerDriver` into the global registry.
    #[must_use]
    pub fn readiness(&self) -> WorkerReadinessState {
        *self.shared.readiness.lock().expect("readiness mutex")
    }

    fn set_readiness(&self, state: WorkerReadinessState) {
        *self.shared.readiness.lock().expect("readiness mutex") = state;
    }

    fn set_replay(&self, replay: Option<SharedReplay>) {
        *self.shared.replay.lock().expect("replay slot mutex") = replay;
    }

    /// Sync readiness predicate answering for itself. The async trait method
    /// delegates here so `mailw`/`raww` keep their sync call without inlining
    /// the rule. Extracted to keep the four ACP readiness call sites from
    /// diverging when the rule gains a condition.
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self.readiness(), WorkerReadinessState::Available)
    }

    /// Releases the live runtime (joining its child) and marks the transport
    /// recovering, clearing the published replay pointer. Closes the write
    /// channel so the internal delivery task drains pending items and exits
    /// cleanly before the runtime is dropped. Used by the worker before a
    /// respawn so a concurrent `look` reads a recovering/stale snapshot through
    /// the still-valid handle rather than the dead buffer.
    pub fn release_runtime(&mut self) {
        // The executor is deliberately left running. It belongs to this transport
        // instance, not to the runtime being released: a respawn installs a
        // replacement connection into the executor that is already there, so
        // there is never a second one to race the first. Telling it the runtime
        // is gone is what makes it withhold writes until one arrives.
        if let Some(runtime_tx) = self.runtime_tx.as_ref() {
            let _ = runtime_tx.send(RuntimeInstall::Clear);
        }
        self.runtime = None;
        self.set_replay(None);
        self.set_readiness(WorkerReadinessState::Recovering);
    }

    /// Starts this transport's one delivery-loop executor, if it has not already
    /// been started.
    ///
    /// Called before the bootstrap rather than after it, and exactly once for the
    /// transport's life. Two things follow from that, and both are the point.
    ///
    /// A bootstrap that fails permanently still leaves an executor running, so
    /// the target's unreachability is observed by something and its queued
    /// entries resolve at the dwell. An executor spawned only alongside a live
    /// client would leave exactly that target's mailbox with no consumer, and
    /// nothing else in the pull model looks at a transport's health.
    ///
    /// A respawn installs a replacement connection into the executor already
    /// running rather than starting another beside it, which is what makes "one
    /// serial executor per transport instance" true by construction instead of by
    /// hoping the previous one has exited.
    fn ensure_executor(&mut self) {
        if self.delivery_task_handle.is_some() {
            return;
        }
        let (runtime_tx, runtime_rx) = std::sync::mpsc::channel::<RuntimeInstall>();
        let respawn_needed_tx = self.respawn_needed_tx.clone();
        let shared = Arc::clone(&self.shared);
        let batch_settings = self.batch_settings;
        let chooser = self.chooser.clone();
        let identity = DeliveryTaskIdentity {
            target_session: self.target_session.clone(),
        };
        let delivery = self.delivery.clone();
        let stop = Arc::clone(&self.executor_stop);
        let reachability = AcpReachability {
            abandoned: Arc::clone(&self.reachability.abandoned),
            unreachable_since: Arc::clone(&self.reachability.unreachable_since),
        };

        let handle = thread::Builder::new()
            .name("agentmux-acp-delivery".into())
            .spawn(move || {
                let writer = AcpDeliveryWriter::new(
                    DeliveryChannels {
                        runtime_rx,
                        stop,
                        respawn_needed_tx,
                    },
                    shared,
                    chooser,
                    batch_settings,
                    identity,
                    reachability,
                );
                run_delivery_executor(writer, delivery);
            })
            .expect("spawn ACP delivery executor thread");

        self.delivery_task_handle = Some(handle);
        self.runtime_tx = Some(runtime_tx);
    }

    /// Hands the established runtime to the executor.
    ///
    /// The client is moved rather than shared: one thread issues every framed
    /// write for this target, which is what makes the executor's seriality reach
    /// the agent rather than stopping at the relay.
    fn hand_runtime_to_executor(&mut self) {
        let runtime = self
            .runtime
            .take()
            .expect("runtime present at executor install");
        // Before the move, not after: this is the last point at which the
        // transport can still reach the client it is about to hand away.
        self.generation = Some(runtime.client.generation_handle());
        let slot = AcpRuntimeSlot {
            client: runtime.client,
            session_id: runtime.session_id,
        };
        if let Some(runtime_tx) = self.runtime_tx.as_ref() {
            let _ = runtime_tx.send(RuntimeInstall::Install(Box::new(slot)));
        }
    }

    /// Sets the chooser/target identity and clears any prior delivery channel
    /// ahead of (re-)establishing the runtime. Brief and lock-safe: it holds no
    /// blocking work, so the driver-owned respawn monitor can call it under the
    /// transport lock without stalling a concurrent `mailw`. Readiness is the
    /// caller's responsibility (initial bootstrap marks Initializing; respawn
    /// leaves the released Recovering state in place).
    pub(crate) fn prepare_for_startup(
        &mut self,
        chooser: crate::transports::Chooser,
        target_session: String,
    ) {
        self.chooser = Some(chooser);
        self.target_session = target_session;
        // The executor starts here, before the bootstrap it will write through
        // rather than after it. A bootstrap that fails permanently leaves the
        // target unreachable, and unreachability is carried by the dwell — which
        // only a running executor observes. Starting it on success instead would
        // leave exactly that target's mailbox with no consumer and its entries
        // with no outcome, which nothing else in the pull model would notice.
        //
        // Idempotent across respawns: the executor is per transport instance, and
        // `prepare_for_startup` runs again before each re-establish.
        self.ensure_executor();
    }

    /// Registers a bootstrap as running until the returned guard drops.
    ///
    /// A bootstrap begun behind an already-terminating generation is registered
    /// like any other rather than refused: it inherits the latch, so the child it
    /// is about to spawn is ended at publication. Refusing here instead would
    /// leave the caller holding no guard and the fence with nothing counting it.
    #[must_use]
    pub(crate) fn begin_bootstrap(&self) -> BootstrapInFlight {
        let id = NEXT_BOOTSTRAP_ID.fetch_add(1, Ordering::Relaxed);
        {
            self.bootstraps.records().push(BootstrapRecord {
                id,
                generation: None,
            });
        }
        // The third registry writer, and it owes the same handoff as the other
        // two. Registration holds the lock a traversal attempt can lose to, and
        // this holder's own record is empty — so serving only itself would serve
        // nothing, while every already-published bootstrap waited on a pass that
        // would not happen until this one reached publication.
        self.bootstraps.serve_pending_termination();
        BootstrapInFlight {
            id,
            bootstraps: Arc::clone(&self.bootstraps),
        }
    }

    /// Whether this generation has been fenced.
    #[must_use]
    pub(crate) fn generation_is_fenced(&self) -> bool {
        self.fenced.load(Ordering::Acquire)
    }

    /// Installs `runtime` unless this generation has been fenced, in which case
    /// it is handed back for the caller to tear down.
    ///
    /// The check and the install happen under one lock on purpose. A bootstrap
    /// runs off the lock and takes seconds, so a fence can begin while one is in
    /// flight; deciding separately from installing would leave the window where
    /// a replacement runtime is installed into a generation already declared
    /// stopped — a second live agent for one target, which is exactly what the
    /// fence exists to prevent.
    #[must_use = "a refused runtime owns a live child and must be shut down"]
    pub(crate) fn install_runtime_unless_fenced(
        &mut self,
        runtime: PersistentAcpWorkerRuntime,
    ) -> Option<PersistentAcpWorkerRuntime> {
        if self.generation_is_fenced() {
            return Some(runtime);
        }
        self.install_runtime(runtime);
        None
    }

    /// Installs a freshly bootstrapped runtime: repoints the published replay
    /// handle at the new buffer, marks the transport Available, and spawns the
    /// internal delivery task. Brief and lock-safe — the blocking child spawn
    /// already happened in `bootstrap_acp_worker_runtime`, so a bootstrap holds
    /// the transport lock only for these fast field updates.
    ///
    /// Private because the fenced check and the install belong together; every
    /// caller goes through [`install_runtime_unless_fenced`].
    ///
    /// [`install_runtime_unless_fenced`]: Self::install_runtime_unless_fenced
    fn install_runtime(&mut self, runtime: PersistentAcpWorkerRuntime) {
        // Repoint the published handle's replay slot at the new runtime's buffer
        // before marking ready, so a look that was prime-waiting through the
        // (re-)establish returns the fresh buffer.
        self.set_replay(Some(runtime.client.replay_buffer_handle()));
        self.runtime = Some(runtime);
        self.set_readiness(WorkerReadinessState::Available);
        // Idempotent, and the executor is usually already running: it is started
        // when the bootstrap begins, not when one succeeds.
        self.ensure_executor();
        self.hand_runtime_to_executor();
    }

    /// Marks the transport Unavailable with no live runtime (initial-bootstrap
    /// failure or permanent respawn give-up).
    pub(crate) fn mark_runtime_unavailable(&mut self) {
        self.runtime = None;
        self.set_replay(None);
        self.set_readiness(WorkerReadinessState::Unavailable);
    }
}

impl GenerationFence for AcpTransport {
    fn fence_generation(&mut self) {
        // Latching the executor's stop flag is its cooperative request: it
        // finishes what it holds and exits at its next check. Marking the
        // generation fenced is the same request to the respawn monitor, and it is
        // what makes a bootstrap already in flight refuse to install.
        self.fenced.store(true, Ordering::Release);
        self.executor_stop.store(true, Ordering::Release);
    }

    fn terminate_generation(&mut self) {
        // Signal the child and return. This unblocks an executor parked writing
        // into the child's stdin, which is exactly the case step 1 cannot reach.
        // Reaping happens in the observation that follows, not here.
        if let Some(generation) = self.generation.as_ref() {
            generation.initiate_termination();
        }
        // Every bootstrap still running owns an agent child of its own, and a
        // bootstrap is held where it is by that child failing to answer. Killing
        // it closes the stdio the handshake is waiting on, so the request fails,
        // the client drops, and the executor returns — where before, step 3 had
        // nothing to say to it and the fence could only wait out an operation
        // timeout it does not control.
        //
        // Latches and attempts one registry-wide pass, blocking nowhere. A
        // record this attempt cannot see, or cannot reach because someone holds
        // the lock, is reached when that holder releases and runs the same pass.
        self.bootstraps.initiate_termination();
    }

    fn generation_ceased(&self) -> bool {
        let delivery_task_ceased = self
            .delivery_task_handle
            .as_ref()
            .is_none_or(JoinHandle::is_finished);
        let client_ceased = self
            .generation
            .as_ref()
            .is_none_or(AcpGenerationHandle::reader_ceased);
        // A record outlives its bootstrap's disposal of the child it owns. On
        // every path that produced no live runtime the client has been dropped
        // by the time the guard goes, and dropping it kills *and waits* the
        // child; on the path that succeeded, that child has become this
        // generation's steady-state one, which the conjunct above observes.
        // Either way an empty registry means reaped or accounted for, never
        // merely signalled.
        let no_bootstrap_running = self.bootstraps.records().is_empty();
        delivery_task_ceased
            && client_ceased
            && no_bootstrap_running
            && self.shared.permission_executors_ceased()
    }
}

impl Transport for AcpTransport {
    /// ACP establishes its runtime through the driver's supervised bootstrap, not
    /// here.
    ///
    /// This used to bootstrap synchronously on the caller's thread: a second
    /// route that spawned and owned an agent child while being counted by
    /// nothing, so a fence could see an empty in-flight count with a live child
    /// coming up behind it. Every production ACP path goes through
    /// [`AcpWorkerDriver::start_bootstrap`], so the route is gone rather than
    /// duplicated under the supervisor.
    fn startup(&mut self, _context: StartupContext) -> Result<TransportStatus, TransportError> {
        Err(TransportError {
            code: "internal_unexpected_failure".to_string(),
            reason: "ACP runtimes are established by the worker driver's supervised bootstrap"
                .to_string(),
            details: None,
        })
    }

    fn health(&self) -> TransportHealth {
        // The inner transport cannot answer this one. `Unavailable` is published
        // for a respawn gap and for a permanent give-up alike, so reading it here
        // would report a recoverable worker as unreachable and bounce messages a
        // respawn was about to make deliverable. Permanence is the driver's
        // knowledge, and `AcpWorkerDriver::health` is what the relay actually
        // calls — `TransportImpl::Acp` holds the driver, never this type.
        TransportHealth::Healthy
    }

    fn shutdown(&mut self) {
        // Signal the executor to finish and exit, then drop the runtime.
        self.executor_stop.store(true, Ordering::Release);
        self.runtime = None;
        self.set_replay(None);
        self.set_readiness(WorkerReadinessState::Unavailable);
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        // Always publishes a handle, even before the first runtime exists: the
        // handle reads the shared state, which the transport repoints across
        // startup/respawn. This keeps the prime-wait reachable during the very
        // windows (initial startup, respawn gap) when there is no live runtime.
        Some(Arc::new(super::output::AcpOutputView {
            shared: Arc::clone(&self.shared),
        }))
    }
}

/// The executor's lifetime is the transport's, not any runtime's.
///
/// Inline because the seam is crate-private by design and no public interface
/// reaches it: `Transport::startup` returns an error for ACP (the driver's
/// supervised bootstrap establishes runtimes instead), so `prepare_for_startup`
/// is the only way an executor is ever spawned, and widening it to `pub` would
/// publish a lifecycle step the driver alone is meant to drive.
///
/// One test because the two claims are one property seen from both ends. A
/// bootstrap that never succeeds and a respawn that replaces a runtime are the
/// same question — does the executor belong to the transport or to the
/// connection — and answering it wrongly breaks them in opposite directions: the
/// first leaves a target's mailbox with no consumer at all, the second leaves it
/// with two.
#[cfg(test)]
mod executor_lifetime_tests {
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::protocol::mailbox::{CursorPosition, EntryRange};
    use crate::protocol::operations::{
        AckRejection, AckResult, DeclareRejection, DeclareResult, MemberAcknowledgment,
        PeekResponse, PeekResult,
    };
    use crate::transports::{DeliveryExecutorContext, MailboxConsumer, PackingUnitId};

    /// A mailbox that is always empty and counts what the executor reports.
    ///
    /// Empty deliberately: what is under test is that an executor exists and
    /// keeps observing, not what it writes. An entry would only add a write path
    /// against an agent that is not there.
    #[derive(Default)]
    struct CountingConsumer {
        peeks: AtomicUsize,
        unreachable_resolutions: AtomicUsize,
    }

    impl MailboxConsumer for CountingConsumer {
        fn peek(&self, _entry_max: usize, _canonical_bytes_max: u64) -> PeekResult {
            self.peeks.fetch_add(1, Ordering::Relaxed);
            Ok(PeekResponse {
                entries: Vec::new(),
                cursor: CursorPosition::start(),
            })
        }

        fn declare(&self, _range: EntryRange) -> DeclareResult {
            Err(DeclareRejection::UnknownTarget)
        }

        fn ack(&self, _unit: PackingUnitId, _members: &[MemberAcknowledgment]) -> AckResult {
            Err(AckRejection::UnknownTarget)
        }

        fn resolve_unreachable(&self) {
            self.unreachable_resolutions.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn wait_for(description: &str, mut condition: impl FnMut() -> bool) {
        let started = Instant::now();
        while !condition() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "timed out waiting for {description}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn one_executor_is_spawned_before_any_runtime_and_survives_a_released_one() {
        let consumer = Arc::new(CountingConsumer::default());
        let abandoned = Arc::new(AtomicBool::new(false));
        let mut transport = AcpTransport::new(
            PromptBatchSettings::default(),
            None,
            DeliveryExecutorContext {
                consumer: Arc::clone(&consumer) as Arc<dyn MailboxConsumer>,
                doorbell: crate::protocol::DeliveryDoorbell::default(),
                poll_interval: Duration::from_millis(5),
                unreachable_dwell: Duration::from_millis(100),
            },
            AcpReachability::new(
                Arc::clone(&abandoned),
                Arc::new(crate::transports::UnreachableSince::default()),
            ),
        );

        // No runtime has been established and none ever will be here. An
        // executor tied to one would not exist at all, and this mailbox would be
        // consumed by nobody — which is the whole defect: nothing else in the
        // pull model looks at a transport's health.
        transport.prepare_for_startup(
            Arc::new(|_| unreachable!("no choice is raised")),
            "t".into(),
        );
        let first = transport
            .delivery_task_handle
            .as_ref()
            .expect("preparing for startup spawns the executor")
            .thread()
            .id();

        // A respawn releases the runtime and prepares again. Neither may start a
        // second executor: two sharing one consumer generation is the seriality
        // violation the single-executor rule exists to prevent. Compared by
        // thread id rather than by the handle merely being present, because a
        // second spawn would replace the handle and leave a `Some` behind it.
        transport.release_runtime();
        transport.prepare_for_startup(
            Arc::new(|_| unreachable!("no choice is raised")),
            "t".into(),
        );
        let after_respawn = transport
            .delivery_task_handle
            .as_ref()
            .expect("the executor survives a released runtime");
        assert_eq!(
            after_respawn.thread().id(),
            first,
            "a respawn must install a runtime into the running executor, never start a second",
        );
        assert!(
            !after_respawn.is_finished(),
            "a released runtime must not end the executor that outlives it",
        );

        // Abandonment is the only thing that makes an ACP target unreachable, and
        // the dwell is carried by the executor. Latching it is what a permanent
        // bootstrap failure does.
        abandoned.store(true, Ordering::Release);
        wait_for("the dwell to resolve the target's entries", || {
            consumer.unreachable_resolutions.load(Ordering::Relaxed) > 0
        });

        // Nothing was consumed along the way. An executor with no runtime must
        // withhold rather than peek: a peek it went on to declare would bind
        // entries it has no connection to write, and the guard would then owe
        // them `submission_unknown` where it could have proven nothing was sent.
        assert_eq!(
            consumer.peeks.load(Ordering::Relaxed),
            0,
            "an executor with no runtime must consume nothing",
        );

        Transport::shutdown(&mut transport);
        wait_for("the executor to stop on shutdown", || {
            transport
                .delivery_task_handle
                .as_ref()
                .is_none_or(std::thread::JoinHandle::is_finished)
        });
    }
}
