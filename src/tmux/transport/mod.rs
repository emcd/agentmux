//! The tmux [`Transport`] implementation and the delivery-loop executor it owns.
//!
//! [`TmuxTransport`] owns exactly one serial delivery-loop executor for its
//! lifetime, spawned during [`startup`](Transport::startup). The relay never
//! invokes this transport to deliver: the executor peeks its target's mailbox,
//! renders what it peeked into pane text, combines a prefix of it into one
//! token-budget-bounded prompt, declares that prompt's entries as a packing unit,
//! pastes it, and acknowledges what the paste proved. Raw entries reach it the
//! same way and are written alone, because the mailbox returns one at the head as
//! a singleton.
//!
//! Tmux sessions are created and owned by the [`lifecycle`](super::lifecycle)
//! primitives (driven by relay bundle reconcile/startup), so the transport owns
//! no session lifecycle. What `startup` owns is the executor and the observer
//! beside it.
//!
//! The observer is a separate thread on purpose. The executor spends most of its
//! time parked on its doorbell, and a readiness reading taken only when it wakes
//! would be a reading of the pane as it was one poll ago; the observer keeps the
//! level current without either thread spawning a tmux client the fence cannot
//! reach.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::configuration::TargetConfiguration;
use crate::envelope::PromptBatchSettings;
use crate::runtime::paths::tmux_socket_path_for_runtime_directory;
use crate::transports::{
    DeliveryEnvelope, DeliveryExecutorContext, GenerationFence, OutputView, StartupContext,
    Transport, TransportError, TransportHealth, TransportReadiness, TransportStatus,
    UnreachableSince, run_delivery_executor,
};

use super::pane::{TmuxInvocationSlot, publish_tmux_invocations, terminate_published_invocation};

mod delivery;
mod executor;
mod observation;
pub use delivery::coalescing_runs;
pub use observation::TmuxOutputView;

/// How often the observer thread re-reads its pane.
///
/// Matches the relay worker's poll cadence: a level staler than the interval at
/// which it is read would make the reader wait for news that had already
/// arrived.
const OBSERVER_INTERVAL_MS: u64 = 100;

/// Marker line written immediately before a terminal-outcome receipt's
/// pane text so the receiving agent can distinguish a relay/system
/// status update from a peer message at a glance. Reuses the same
/// literal as the Pty transport (`src/pty/delivery.rs`) for
/// cross-transport consistency.
const RECEIPT_MARKER: &str = "--- agentmux terminal-outcome receipt ---";

/// Renders one envelope's pane text for tmux paste. Receipt envelopes
/// (`DeliveryEnvelope.is_receipt`) get a leading marker line so the
/// receiving agent can distinguish a relay/system status update from
/// a peer message at a glance. The marker is included in the
/// rendered text so the token-budget batching and paste-budget counts
/// stay consistent with the actual pane bytes.
///
/// Detection uses the typed `DeliveryEnvelope.is_receipt` field the
/// relay's terminal-resolution chokepoint propagates from
/// `AsyncDeliveryTask.is_receipt`; no Tmux-side sender identity
/// inference. Receipts are non-recursive at the relay-side chokepoint;
/// the Tmux transport does not enforce or check that invariant.
pub fn render_paste_text(envelope: &DeliveryEnvelope) -> String {
    let body = envelope.message.render_pane_envelope(&envelope.message_id);
    if envelope.is_receipt {
        format!("{RECEIPT_MARKER}\n{body}")
    } else {
        body
    }
}

/// Context captured at `startup` for the delivery-loop executor and the observer.
#[derive(Clone)]
struct DeliveryTaskContext {
    target_session: String,
    runtime_directory: PathBuf,
    target_member: crate::configuration::BundleMember,
}

/// Tmux pane delivery transport with one serial delivery-loop executor.
///
/// The executor is spawned at `startup` and lives for the transport instance's
/// lifetime. Nothing is handed to this transport to deliver; it consumes its
/// target's mailbox itself.
pub struct TmuxTransport {
    batch_settings: PromptBatchSettings,
    /// What the relay injected so the executor can reach its target's mailbox:
    /// the consumer handle, the doorbell, and the two `[delivery]` durations it
    /// paces itself by. Consumed at `startup`, which is where the executor that
    /// drives it is spawned.
    delivery: DeliveryExecutorContext,
    executor_handle: Option<thread::JoinHandle<()>>,
    task_context: Option<DeliveryTaskContext>,
    shutdown_flag: Arc<AtomicBool>,
    /// The tmux client invocation the delivery thread is currently waiting on.
    ///
    /// Retained so the fence's forced step has something to signal: dropping the
    /// channel reaches a thread between items, and nothing else reaches one
    /// parked in a tmux client call.
    invocation: TmuxInvocationSlot,
    /// Latch for the health axis: when the pane first stopped being observable.
    ///
    /// Shared with the executor rather than duplicated, so the level the
    /// executor measures its dwell against and the level this transport reports
    /// cannot disagree about when unreachability began. A second latch would
    /// give the two different clocks for the same condition.
    unreachable_since: Arc<UnreachableSince>,
    /// The most recent pane observation, published by the observer thread and
    /// read by the two contract predicates.
    ///
    /// The relay reads levels from its async worker, so the read must not block
    /// and must not spawn a tmux client. It also must not spawn one *there*
    /// specifically: `publish_tmux_invocations` is thread-local, so an
    /// invocation made from a runtime worker thread lands in no slot and
    /// `terminate_generation` cannot reach it. Observing on an owned thread that
    /// publishes into a slot fixes both at once.
    observation: Arc<Mutex<PaneObservation>>,
    /// The observer thread's own invocation slot.
    ///
    /// Separate from the delivery thread's: a slot holds one child, so two
    /// publishers would clobber each other. The fence signals both.
    observer_invocation: TmuxInvocationSlot,
    /// Handle to the observer thread, for cessation observation and shutdown.
    observer_handle: Option<thread::JoinHandle<()>>,
    /// Relay-provided closure the observer invokes when its observation changes.
    ///
    /// An opaque `Arc<dyn Fn>` supplied at construction, the same shape Pty uses
    /// for `mirror_state`, so this module names no relay type. Correctness never
    /// depends on it: the authoritative state is the level the relay reads, and a
    /// lost invocation only delays a delivery to the next poll.
    readiness_notifier: Option<ReadinessNotifier>,
}

/// Relay-provided wakeup invoked when a transport's observed level changes.
pub type ReadinessNotifier = Arc<dyn Fn() + Send + Sync>;

/// The most recent thing the observer thread learned about the pane.
#[derive(Clone, Copy, Debug)]
enum PaneObservation {
    /// Nothing observed yet. Deliberately distinct from `Unreachable`: not having
    /// looked is ignorance, not evidence, and reporting it as unreachable would
    /// start a dwell against a target nobody has examined.
    Pending,
    /// The pane could not be inspected at all — a departed session, a dead
    /// server, or a delivery task that has stopped.
    Unreachable,
    /// The pane was inspected, and this is what it said.
    Observed {
        ready: bool,
        /// tmux's window-activity marker at that observation, or `0` when the
        /// format is unavailable. Carried but never classified on here: the
        /// delivery loop compares consecutive values, because only it knows
        /// which two observations bracket a write decision.
        activity: u64,
    },
}

impl PaneObservation {
    /// Whether replacing `self` with `next` changes either axis the relay reads.
    ///
    /// Activity is deliberately **not** an axis for this purpose. It advances on
    /// every byte the target writes, so counting it would fire the notifier
    /// continuously through an agent's turn — turning a wakeup that exists to
    /// replace polling into a second, faster poll. The relay's own tick is what
    /// re-reads activity; this closure exists for the transitions that are rare
    /// and worth waking for.
    fn differs_from(self, next: Self) -> bool {
        !matches!(
            (self, next),
            (Self::Pending, Self::Pending)
                | (Self::Unreachable, Self::Unreachable)
                | (
                    Self::Observed { ready: true, .. },
                    Self::Observed { ready: true, .. }
                )
                | (
                    Self::Observed { ready: false, .. },
                    Self::Observed { ready: false, .. }
                )
        )
    }

    /// The activity marker, or `0` for an observation that has none.
    fn activity(self) -> u64 {
        match self {
            Self::Observed { activity, .. } => activity,
            Self::Pending | Self::Unreachable => 0,
        }
    }
}

impl std::fmt::Debug for TmuxTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TmuxTransport")
            .field("batch_settings", &self.batch_settings)
            .field(
                "executor_running",
                &self
                    .executor_handle
                    .as_ref()
                    .map(|handle| !handle.is_finished()),
            )
            .finish()
    }
}

impl TmuxTransport {
    /// Takes the readiness notifier at construction rather than through a
    /// setter. `startup` spawns the observer, and the observer captures the
    /// notifier as it starts; anything that installs the closure afterwards
    /// hands it to a thread that has already taken its copy, leaving a wakeup
    /// path that is wired at every point except the one that fires it. Passing
    /// it in makes that ordering unrepresentable instead of merely documented.
    #[must_use]
    pub fn new(
        batch_settings: PromptBatchSettings,
        readiness_notifier: Option<ReadinessNotifier>,
        delivery: DeliveryExecutorContext,
    ) -> Self {
        Self {
            batch_settings,
            readiness_notifier,
            delivery,
            executor_handle: None,
            task_context: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            invocation: TmuxInvocationSlot::default(),
            unreachable_since: Arc::new(UnreachableSince::default()),
            observation: Arc::new(Mutex::new(PaneObservation::Pending)),
            observer_invocation: TmuxInvocationSlot::default(),
            observer_handle: None,
        }
    }

    /// Starts the observer thread if it is not already running.
    ///
    /// Owned rather than folded into the delivery thread because that thread
    /// blocks on its channel: it observes nothing while idle, which is exactly
    /// when the relay is holding a member and needs a current level.
    fn ensure_observer_running(&mut self) {
        if self
            .observer_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return;
        }
        let Some(context) = self.task_context.clone() else {
            return;
        };
        let TargetConfiguration::Tmux(target) = context.target_member.target.clone() else {
            return;
        };
        let socket = tmux_socket_path_for_runtime_directory(context.runtime_directory.as_path());
        let observation = Arc::clone(&self.observation);
        let slot = Arc::clone(&self.observer_invocation);
        let shutdown_flag = Arc::clone(&self.shutdown_flag);
        let target_session = context.target_session.clone();
        let notifier = self.readiness_notifier.clone();
        self.observer_handle = Some(thread::spawn(move || {
            // Publish before the first invocation, so every tmux client this
            // thread spawns is reachable by the fence.
            publish_tmux_invocations(slot);
            while !shutdown_flag.load(Ordering::Acquire) {
                let observed = observation::observe_pane_once(
                    socket.as_path(),
                    target_session.as_str(),
                    target.prompt_readiness.as_ref(),
                );
                let next = match observed {
                    Some((ready, activity)) => PaneObservation::Observed { ready, activity },
                    None => PaneObservation::Unreachable,
                };
                // Notify on the edge only. A wakeup per tick would make the
                // closure a second poll rather than a substitute for one, and the
                // relay re-reads the level itself on waking.
                let changed = {
                    let mut current = observation.lock().expect("tmux observation mutex");
                    let changed = current.differs_from(next);
                    *current = next;
                    changed
                };
                if changed && let Some(notifier) = notifier.as_ref() {
                    notifier();
                }
                thread::sleep(Duration::from_millis(OBSERVER_INTERVAL_MS));
            }
        }));
    }

    /// One pane observation, separating "could not observe" from "observed".
    ///
    /// `None` means the pane could not be inspected at all — the delivery task
    /// is gone, the target is not a tmux target, or the tmux client call failed,
    /// which is what happens when the session or its server has departed.
    /// `Some(ready)` means the pane was inspected and this is what it said.
    ///
    /// The two readings feed different axes and must not be collapsed: an
    /// unobservable pane is not a busy one, and only the busy one is worth
    /// waiting on.
    /// The cached observation, plus the liveness fact only the transport holds.
    ///
    /// A stopped observer is unreachable however healthy the pane last looked:
    /// the reading below would be frozen at whatever it said when the thread
    /// went, and reporting a stale `ready` would hand the executor a level
    /// nothing is refreshing.
    fn observed(&self) -> PaneObservation {
        let observing = self
            .observer_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished());
        if !observing {
            return PaneObservation::Unreachable;
        }
        *self.observation.lock().expect("tmux observation mutex")
    }

    /// Spawns this transport's one serial delivery-loop executor.
    ///
    /// One per transport instance, for its lifetime. It is spawned rather than
    /// started lazily on a first write because there is no first write to start
    /// it: nothing is handed to this transport, so an executor that waited to be
    /// asked would never run at all.
    fn spawn_executor(&mut self, context: &DeliveryTaskContext) {
        let delivery = self.delivery.clone();
        let writer = executor::tmux_delivery_writer(
            context.runtime_directory.as_path(),
            context.target_session.clone(),
            self.batch_settings,
            Arc::clone(&self.observation),
            Arc::clone(&self.unreachable_since),
            Arc::clone(&self.shutdown_flag),
        );
        let invocation = Arc::clone(&self.invocation);
        self.executor_handle = Some(thread::spawn(move || {
            // Published before the first invocation, so every tmux client this
            // thread spawns is reachable by the fence's forced step.
            publish_tmux_invocations(invocation);
            run_delivery_executor(writer, delivery);
        }));
    }
}

impl GenerationFence for TmuxTransport {
    fn fence_generation(&mut self) {
        // The delivery thread already checks this flag between write items, so
        // marking it is the whole cooperative request.
        self.shutdown_flag.store(true, Ordering::Release);
    }

    fn terminate_generation(&mut self) {
        // The cooperative flag reaches a thread between its own checks; it does
        // nothing at all for one blocked inside a tmux client call, which is the
        // case that made step 1 fail in the first place. Signalling the
        // invocation is what unblocks that thread so the observation after this
        // can succeed.
        //
        // The tmux **server** is deliberately untouched. It is not owned by this
        // generation — it holds the operator's sessions, and terminating it to
        // fence one delivery would destroy work the fence exists to protect.
        terminate_published_invocation(&self.invocation);
        // The observer holds a second slot, and a fence that reached only the
        // executor would leave a tmux client of this generation running.
        terminate_published_invocation(&self.observer_invocation);
    }

    fn generation_ceased(&self) -> bool {
        // A generation that never started an executor owns none and has
        // trivially ceased.
        self.executor_handle
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
            && self
                .observer_handle
                .as_ref()
                .is_none_or(thread::JoinHandle::is_finished)
    }
}

impl Transport for TmuxTransport {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        let task_context = DeliveryTaskContext {
            target_session: context.target_member.id.clone(),
            runtime_directory: context.runtime_directory,
            target_member: context.target_member,
        };
        self.task_context = Some(task_context.clone());
        // The observer starts first, so the executor's first readiness reading
        // has something to read. It would answer `Pending` either way and simply
        // hold, but starting them the other way round would make the first
        // delivery to every target wait a poll interval for no reason.
        self.ensure_observer_running();
        self.spawn_executor(&task_context);
        Ok(TransportStatus {
            readiness: TransportReadiness::Ready,
        })
    }

    fn activity_generation(&self) -> u64 {
        self.observed().activity()
    }

    fn health(&self) -> TransportHealth {
        // Reads the same cached observation and the same latch the executor does,
        // so the two cannot disagree about reachability or about when it began.
        //
        // `Pending` reports healthy. Not having observed yet is ignorance, not
        // evidence, and starting a dwell against a target nobody has examined
        // would resolve members on the strength of not knowing. Such a target
        // reports unready through the other axis, so its entries are held.
        let reachable = !matches!(self.observed(), PaneObservation::Unreachable);
        self.unreachable_since.fold(reachable)
    }

    fn shutdown(&mut self) {
        // The flag is what ends both threads; they read it between units and
        // between observations.
        self.shutdown_flag.store(true, Ordering::Release);
        // The handles are deliberately kept. A dropped handle reads as ceased,
        // and reporting cessation while a thread is still between its stop check
        // and its return is the answer that lets a replacement generation start
        // alongside a live writer.
        self.task_context = None;
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        None
    }
}

#[cfg(test)]
mod observation_edge_tests {
    use super::PaneObservation;

    /// The notifier fires on change, so what counts as a change is the contract.
    ///
    /// Firing every tick would make the closure a second poll rather than a
    /// substitute for one; never firing on a real transition would leave a held
    /// member waiting out the backstop interval it exists to avoid.
    #[test]
    fn an_observation_differs_only_when_an_axis_the_relay_reads_moves() {
        // An activity marker moving under an unchanged `ready` is deliberately
        // not a change: it advances on every byte the target writes, so waking
        // the relay for it would replace one poll with a faster one.
        assert!(
            !PaneObservation::Observed {
                ready: false,
                activity: 7,
            }
            .differs_from(PaneObservation::Observed {
                ready: false,
                activity: 9,
            }),
            "activity alone must not wake the relay"
        );

        let states = [
            PaneObservation::Pending,
            PaneObservation::Unreachable,
            PaneObservation::Observed {
                ready: false,
                activity: 0,
            },
            PaneObservation::Observed {
                ready: true,
                activity: 0,
            },
        ];
        for (index, current) in states.iter().enumerate() {
            for (other, next) in states.iter().enumerate() {
                assert_eq!(
                    current.differs_from(*next),
                    index != other,
                    "{current:?} -> {next:?} should {} a change",
                    if index == other { "not be" } else { "be" }
                );
            }
        }
    }
}
