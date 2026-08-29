//! Pty transport core.
//!
//! [`PtyTransport`] is the per-target
//! [`Transport`](crate::transports::Transport) implementation. The
//! transport does NOT own the libghostty-vt terminal directly — the
//! terminal is constructed and lives entirely on the worker thread.
//!
//! Channels the transport owns:
//!
//! - `write_tx`: the relay submits [`DeliveryCommand`]s into this
//!   channel; the worker drains them.
//! - `bytes_tx`: the reader thread feeds terminal output bytes into
//!   this channel; the worker feeds them into the terminal.
//!
//! Channels the worker thread owns:
//!
//! - `bytes_rx` (terminal -> delivery task): feeds terminal output
//!   bytes (rendered as snapshots on demand).
//! - `write_rx` (relay -> delivery task): drains delivery commands.
//! - `snapshot_rx` (look / probe -> delivery task): routes snapshot
//!   requests through the delivery task.
//!
//! The `libghostty-vt` terminal is `!Send + !Sync` (raw FFI pointers +
//! `dyn` trait object callbacks), so it must be constructed and live
//! entirely on the worker thread. The cross-thread coordination
//! between the relay's look path and the worker goes through the
//! snapshot channel; the cross-thread coordination between the reader
//! thread and the worker goes through the bytes channel.
//!
//! Status: the Transport trait surface compiles against the
//! libghostty-vt binding. Delivery writes are resolved from the PTY master
//! result after handover admission. Per-coder config parsing
//! (`[coders.<id>.pty]` → [`PtyTargetConfiguration`]) lands in §6.
//! The `TransportImpl::pty` cfg-gated wiring lands in §5.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use regex::Regex;
use tokio::sync::{mpsc, oneshot};

use crate::configuration::BundleMember;
use crate::configuration::TermProtocol;
use crate::transports::{
    DeliveryEnvelope, GenerationFence, OutcomeFuture, OutputView, PartitionSink,
    SingleDeliveryOutcome, StartupContext, Transport, TransportError, TransportHealth,
    TransportReadiness, TransportStatus, UnreachableSince, WorkerReadinessState,
};

/// Mirrors the worker readiness state into the relay's global registry.
///
/// Constructed relay-side closing over `set_worker_readiness(namespace,
/// runtime_directory, target_session, state)` (see
/// `src/relay/delivery/async_worker.rs`); the transport holds an opaque
/// `Arc<dyn Fn>` so `src/pty` does not import `crate::relay`. `None` in
/// tests that construct the transport without a relay registry.
///
/// Mirrors ACP's `MirrorStateFn` (see `src/acp/worker_driver.rs`).
pub type PtyMirrorStateFn = Arc<dyn Fn(WorkerReadinessState) + Send + Sync>;

use super::state::{PtyConfigSnapshot, PtyOutputView, PtyPromptProbe, PtyShared};

mod lifecycle;
mod runtime;

/// Default pty cols when the per-coder config does not set them.
pub const DEFAULT_COLS: u16 = 120;
/// Default pty rows when the per-coder config does not set them.
pub const DEFAULT_ROWS: u16 = 40;
/// Capacity of the write-channel the relay submits into.
const WRITE_CHANNEL_CAPACITY: usize = 256;
/// Poll interval when the worker has no pending work across any
/// channel.
const WORKER_IDLE_POLL: Duration = Duration::from_millis(10);
/// Poll interval for the reader thread when the master returns
/// `WouldBlock`.
const READER_IDLE_POLL: Duration = Duration::from_millis(5);
/// Bound on the prompt-probe handshake in the handover gate.
///
/// The probe does `snapshot_tx.send().await + rx.await` through the worker
/// thread. If that worker never answers, the gate must not park the delivery
/// worker forever — the gate's own doc says the reading is advisory and may be
/// stale, so a timeout answer as not-ready (Hold) is consistent. The poll arm
/// retries and the health dwell carries to Unreachable if the target is truly gone.
const PTY_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Bound on how long a partial-startup cleanup may wait for a thread to finish
/// before giving up on joining it.
///
/// A worker thread stalled inside the uninterruptible
/// `libghostty_vt::Terminal::new` can never be forced to return, so a cleanup
/// that must not hang startup observes `is_finished()` within this bound and
/// then detaches rather than blocking. The child is already killed and reaped,
/// and the shutdown flag is latched, so a detached worker that eventually
/// returns exits without publishing stale readiness.
const STARTUP_CLEANUP_JOIN_BOUND: Duration = Duration::from_secs(2);
/// Poll cadence for the bounded startup-cleanup observation.
const STARTUP_CLEANUP_POLL: Duration = Duration::from_millis(10);
/// Agentmux-pty version string returned by the `on_xtversion`
/// callback. The relay-tui can display it for the operator.
const PTY_VERSION_STRING: &str = concat!("agentmux-pty ", env!("CARGO_PKG_VERSION"));

type PtyChildHandle = Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>;

/// Per-coder pty target configuration as parsed from
/// `[coders.<id>.pty]`. The full config surface lands in §6.
#[derive(Clone, Debug)]
pub struct PtyTargetConfiguration {
    pub initial_command: String,
    pub resume_command: String,
    /// Prompt-readiness template (regex + optional inspect lines +
    /// optional idle cursor column). Mirrors the
    /// `prompt_readiness` field of the validated
    /// `config::types::PtyTargetConfiguration`.
    pub prompt_readiness: Option<crate::configuration::PromptReadinessTemplate>,
    pub cols: u16,
    pub rows: u16,
    /// Optional working directory. When set, the worker `chdir`s
    /// into it before spawning the child. When `None`, the worker
    /// uses the bundle's runtime directory (the relay passes that
    /// in via `StartupContext::runtime_directory`).
    pub working_directory: Option<std::path::PathBuf>,
    /// Per-coder terminal protocol; selects the literal `TERM`
    /// env-var value the transport sets when spawning the child.
    /// Defaults to `xterm-256color` when absent.
    pub term_protocol: TermProtocol,
}

/// Internal command the relay submits via `mailw` / `raww`.
#[allow(clippy::large_enum_variant)]
pub enum DeliveryCommand {
    Envelope {
        envelope: Box<DeliveryEnvelope>,
        outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
    },
    Raw {
        content: String,
        append_enter: bool,
        outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
    },
}

/// Pty pane delivery transport with an internal delivery task.
///
/// The transport owns an ordered channel carrying
/// [`DeliveryCommand`]s. The relay worker submits writes via
/// `mailw`/`raww` without blocking; a worker thread drains the
/// channels, processes PTY output, services snapshot requests, and
/// executes delivery commands.
pub struct PtyTransport {
    target_member: BundleMember,
    shared: PtyShared,
    /// True after the first `startup()` call has returned, whether
    /// successfully or with `Err`.
    started: bool,
    /// Configured initial command (from the per-coder
    /// `[coders.<id>.pty].initial-command` after the bootstrap path
    /// substitutes `{coder-session-id}`). Used by `startup` to launch
    /// the child process.
    configured_initial_command: String,
    /// Configured working directory (from the bundle member's
    /// `working_directory` or the per-coder
    /// `[coders.<id>.pty].working-directory`). When `None`, the
    /// worker uses the runtime directory the relay passes via
    /// `StartupContext::runtime_directory`.
    configured_working_directory: Option<std::path::PathBuf>,
    /// Configured terminal protocol; selects the literal `TERM`
    /// env-var value the transport sets when spawning the child.
    /// Defaults to `xterm-256color` when the per-coder config omits
    /// `term-protocol`.
    configured_term_protocol: TermProtocol,
    /// Write-command channel the relay submits into. `None` before
    /// `startup`; `Some` once the worker thread is running.
    write_tx: Option<mpsc::Sender<DeliveryCommand>>,
    /// Bytes channel the reader thread feeds into the worker.
    /// `None` before `startup`; `Some` once the worker thread is running.
    bytes_tx: Option<mpsc::Sender<Vec<u8>>>,
    /// Set to `true` by `shutdown` so the worker / reader threads
    /// drain and exit cleanly.
    shutdown_flag: Arc<AtomicBool>,
    /// Handle to the worker thread. Joined by `shutdown`.
    worker_handle: Option<thread::JoinHandle<()>>,
    /// Handle to the reader thread. Joined by `shutdown`.
    reader_handle: Option<thread::JoinHandle<()>>,
    /// Latch for the health axis; see [`Transport::health`].
    unreachable_since: UnreachableSince,
    /// Live handle to the spawned child. `shutdown` kills and reaps.
    child: Option<Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>,
    /// Per-transport readiness state.
    readiness: Arc<Mutex<WorkerReadinessState>>,
    /// Optional relay-provided closure that mirrors per-turn readiness
    /// transitions into the relay's global worker-state registry.
    mirror_state: Option<PtyMirrorStateFn>,
    /// The relay's guard, for reporting which member each write covers.
    partition_sink: Arc<dyn PartitionSink>,
}

impl std::fmt::Debug for PtyTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyTransport")
            .field("target_member", &self.target_member)
            .finish_non_exhaustive()
    }
}

impl PtyTransport {
    /// Constructs a new PtyTransport with the given target member and
    /// per-coder configuration.
    ///
    /// The per-coder values split by who needs them. The target id, grid
    /// dimensions, and compiled prompt-readiness template go into the shared
    /// `PtyConfigSnapshot`, because the runtime probe reads them from both the
    /// worker thread and cross-thread callers and both must see the same
    /// values. The start command, working directory, and TERM protocol stay on
    /// the transport itself: they are consumed once, at child spawn, and the
    /// probe has no use for them.
    ///
    /// `mirror_state` is the relay-constructed closure that mirrors
    /// per-turn readiness transitions into the relay's global
    /// worker-state registry. Pass `None` in tests constructed
    /// without a relay registry; the transport's internal readiness
    /// state still advances so the readiness predicates can drive the
    /// lifecycle locally.
    #[must_use]
    pub fn new(
        target_member: BundleMember,
        config: PtyTargetConfiguration,
        mirror_state: Option<PtyMirrorStateFn>,
        partition_sink: Arc<dyn PartitionSink>,
    ) -> Self {
        let (prompt_regex, prompt_inspect_lines, prompt_idle_column) =
            match config.prompt_readiness.as_ref() {
                Some(pr) => {
                    let compiled = Regex::new(&pr.prompt_regex).ok();
                    (
                        compiled,
                        pr.inspect_lines
                            .map_or(3, |n| u16::try_from(n).unwrap_or(3)),
                        pr.input_idle_cursor_column
                            .and_then(|c| u16::try_from(c).ok()),
                    )
                }
                None => (None, 3, None),
            };
        let shared = PtyShared {
            config: PtyConfigSnapshot {
                target_member_id: target_member.id.clone(),
                cols: config.cols,
                rows: config.rows,
                prompt_regex,
                prompt_inspect_lines,
                prompt_idle_column,
            },
            snapshot_tx: mpsc::channel(64).0,
            child_exited: Arc::new(AtomicBool::new(false)),
        };
        Self {
            target_member,
            shared,
            partition_sink,
            started: false,
            configured_initial_command: config.initial_command.clone(),
            configured_working_directory: config.working_directory.clone(),
            configured_term_protocol: config.term_protocol,
            write_tx: None,
            bytes_tx: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            unreachable_since: UnreachableSince::default(),
            worker_handle: None,
            reader_handle: None,
            child: None,
            readiness: Arc::new(Mutex::new(WorkerReadinessState::Initializing)),
            mirror_state,
        }
    }

    /// Update the readiness state and (if a closure was injected)
    /// mirror the transition into the relay's global worker-state
    /// registry. Used at every relay-side lifecycle point: the
    /// pre-init `Initializing` publish, the post-failure `Unavailable`
    /// publish, and the pre-shutdown `Unavailable` publish.
    ///
    /// The worker thread publishes the rest of the lifecycle
    /// (`Available` on successful init, `Busy`/`Available`/
    /// `Unavailable` around each delivery) via the cloned
    /// `Arc<Mutex<WorkerReadinessState>>` + cloned `mirror_state`
    /// closure it owns.
    ///
    /// `Recovering` transitions are not emitted by this transport —
    /// they require a respawn monitor that does not yet exist for Pty
    /// (deferred to the bootstrap-side wiring follow-up). The live
    /// `WorkerReadinessState` enum retains the variant; only Pty's
    /// emissions of it are deferred.
    fn set_readiness(&self, state: WorkerReadinessState) {
        runtime::publish(&self.readiness, self.mirror_state.as_ref(), state);
    }

    /// Read the current readiness state. Used by
    /// [`has_live_runtime`](Self::has_live_runtime), by
    /// [`Transport::is_ready_for_handover`], and by tests asserting lifecycle
    /// transitions.
    #[must_use]
    pub fn readiness(&self) -> WorkerReadinessState {
        *self.readiness.lock().expect("pty readiness mutex")
    }

    /// Whether the worker runtime exists and is usable, which `Busy` satisfies:
    /// a pty mid-turn still has a live master to snapshot.
    ///
    /// Deliberately not the handover question. [`Transport::is_ready_for_handover`]
    /// asks whether the target can take a turn *now* and excludes `Busy`; gating
    /// the output view on that stricter reading would withhold the `look` surface
    /// from exactly the target most worth looking at.
    fn has_live_runtime(&self) -> bool {
        matches!(
            self.readiness(),
            WorkerReadinessState::Available | WorkerReadinessState::Busy
        )
    }

    /// Update the prompt-readiness settings after construction. Used
    /// when the bootstrap path constructs the transport with placeholder
    /// defaults and the dispatcher (Chunk 4) provides the resolved
    /// per-coder config. Stored on the shared `PtyConfigSnapshot` so
    /// both the cross-thread and the worker-local probe see it.
    pub fn with_prompt_readiness(
        &mut self,
        prompt_readiness: Option<crate::configuration::PromptReadinessTemplate>,
    ) {
        let (prompt_regex, prompt_inspect_lines, prompt_idle_column) =
            match prompt_readiness.as_ref() {
                Some(pr) => {
                    let compiled = Regex::new(&pr.prompt_regex).ok();
                    (
                        compiled,
                        pr.inspect_lines
                            .map_or(3, |n| u16::try_from(n).unwrap_or(3)),
                        pr.input_idle_cursor_column
                            .and_then(|c| u16::try_from(c).ok()),
                    )
                }
                None => (None, 3, None),
            };
        self.shared.config.prompt_regex = prompt_regex;
        self.shared.config.prompt_inspect_lines = prompt_inspect_lines;
        self.shared.config.prompt_idle_column = prompt_idle_column;
    }
}

impl PtyTransport {
    /// Whether this generation's child has been reaped, which the fence's second
    /// observation requires alongside the executors having returned.
    ///
    /// The threads returning is not the whole answer: a live child still holds
    /// the pty and can still write to it, so a generation whose child survives
    /// has not ceased in the sense the fence asks about. Nothing before forced
    /// termination can make this true, which is why a Pty fence always escalates
    /// — the honest reading of what is still running, not a missing cooperative
    /// path.
    ///
    /// `try_wait` reaps without blocking and `try_lock` keeps the observation
    /// itself non-blocking: a lock held by a concurrent teardown reads as
    /// not-yet-ceased, and the next poll asks again.
    fn child_reaped(&self) -> bool {
        let Some(child) = self.child.as_ref() else {
            return true;
        };
        let Ok(mut child) = child.try_lock() else {
            return false;
        };
        matches!(child.try_wait(), Ok(Some(_)))
    }
}

impl GenerationFence for PtyTransport {
    fn fence_generation(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
    }

    fn terminate_generation(&mut self) {
        // Signal the child and return. Killing it closes the pty master, which
        // is what wakes an executor blocked in a `write_all` into that master —
        // the case the cooperative flag cannot reach, because a thread parked in
        // a syscall checks nothing.
        //
        // Deliberately no `wait()` here, unlike `shutdown`: reaping is an
        // observation, and observations belong to the bounded step that follows
        // rather than inside a call contracted to return without blocking.
        if let Some(child_arc) = self.child.as_ref()
            && let Ok(mut child) = child_arc.lock()
        {
            let _ = child.kill();
        }
        self.write_tx = None;
        self.bytes_tx = None;
    }

    fn generation_ceased(&self) -> bool {
        let executors_returned = self
            .worker_handle
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
            && self
                .reader_handle
                .as_ref()
                .is_none_or(thread::JoinHandle::is_finished);
        executors_returned && self.child_reaped()
    }
}

impl Transport for PtyTransport {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        // Re-init is gated by the `started` flag + readiness state.
        //
        // `started == false` is the never-attempted constructor
        // state. The constructor leaves readiness at `Initializing`,
        // so a guard keyed only on readiness would reject the first
        // legitimate `startup()` call — `started` distinguishes
        // never-attempted from in-progress.
        //
        // `started == true && readiness in {Available, Busy}` is a
        // live transport; the no-op fast path returns
        // `TransportStatus::Ready` so callers can re-invoke
        // `startup()` after a `Drop` of a held `TransportImpl`
        // without consequence.
        //
        // `started == true && readiness in {Initializing,
        // Unavailable, Recovering}` is rejected: a re-attempt
        // against an in-progress startup would race with the
        // existing call; a re-attempt after `Unavailable` (prior
        // shutdown / child-exit / init-failure) is unsupported
        // until a teardown-then-restart path lands.
        if self.started {
            match self.readiness() {
                WorkerReadinessState::Available | WorkerReadinessState::Busy => {
                    return Ok(TransportStatus {
                        readiness: TransportReadiness::Ready,
                    });
                }
                WorkerReadinessState::Initializing => {
                    return Err(TransportError {
                        code: "pty_startup_in_progress".to_string(),
                        reason: "Pty transport startup is already in progress".to_string(),
                        details: None,
                    });
                }
                WorkerReadinessState::Unavailable => {
                    return Err(TransportError {
                        code: "pty_unavailable_restart_unsupported".to_string(),
                        reason:
                            "Pty transport is in Unavailable state after a prior shutdown or child exit; \
                             restart is not yet supported (await respawn-monitor follow-up)"
                                .to_string(),
                        details: None,
                    });
                }
                WorkerReadinessState::Recovering => {
                    return Err(TransportError {
                        code: "pty_recovering_restart_unsupported".to_string(),
                        reason:
                            "Pty transport is in Recovering state; restart is not yet supported"
                                .to_string(),
                        details: None,
                    });
                }
            }
        }

        self.set_readiness(WorkerReadinessState::Initializing);

        let result = self.startup_inner(&context);
        // Mark the attempt complete regardless of outcome so a
        // subsequent call observes `started == true` and consults
        // the readiness state (not the never-attempted path).
        self.started = true;

        if result.is_err() {
            // `startup_inner`'s `StartupGuard::Drop` cleaned up any
            // partial resources; the worker (if it reached the
            // publish step) may have published `Unavailable` on its
            // way out. Ensure the transport-local readiness reflects
            // the failure even if neither cleanup ran (very early
            // failure before any thread / child resource existed).
            if !matches!(self.readiness(), WorkerReadinessState::Unavailable) {
                self.set_readiness(WorkerReadinessState::Unavailable);
            }
        }
        result
    }

    fn mailw(&mut self, envelope: DeliveryEnvelope) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let Some(write_tx) = self.write_tx.clone() else {
            let _ = outcome_tx.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: envelope.message_id.clone(),
                outcome: crate::transports::SendOutcome::Failed,
                reason_code: Some("transport_not_started".to_string()),
                reason: Some("mailw called before startup()".to_string()),
                details: None,
            });
            return outcome_rx;
        };
        let cmd = DeliveryCommand::Envelope {
            envelope: Box::new(envelope),
            outcome_tx,
        };
        // Non-blocking submission: the relay documents this write seam as
        // non-blocking (src/relay/delivery/dispatch/worker.rs), and the
        // worker design rests on that holding. A full channel must not park
        // a delivery-runtime worker thread. On a full or closed channel the
        // item comes back unchanged; resolve the outcome immediately with a
        // terminal failure so the relay's collector never waits on it.
        if let Err(error) = write_tx.try_send(cmd) {
            let DeliveryCommand::Envelope {
                envelope,
                outcome_tx,
            } = error.into_inner()
            else {
                unreachable!("mailw only enqueues Envelope commands");
            };
            let _ = outcome_tx.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: envelope.message_id.clone(),
                outcome: crate::transports::SendOutcome::Failed,
                reason_code: Some("channel_full".to_string()),
                reason: Some("pty internal write channel full or closed".to_string()),
                details: None,
            });
        }
        outcome_rx
    }

    fn raww(&mut self, content: String, append_enter: bool) -> OutcomeFuture {
        let (outcome_tx, outcome_rx) = oneshot::channel();
        let Some(write_tx) = self.write_tx.clone() else {
            let _ = outcome_tx.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: String::new(),
                outcome: crate::transports::SendOutcome::Failed,
                reason_code: Some("transport_not_started".to_string()),
                reason: Some("raww called before startup()".to_string()),
                details: None,
            });
            return outcome_rx;
        };
        let cmd = DeliveryCommand::Raw {
            content,
            append_enter,
            outcome_tx,
        };
        // Non-blocking submission; see `mailw` for the contract.
        if let Err(error) = write_tx.try_send(cmd) {
            let DeliveryCommand::Raw { outcome_tx, .. } = error.into_inner() else {
                unreachable!("raww only enqueues Raw commands");
            };
            let _ = outcome_tx.send(SingleDeliveryOutcome {
                target_session: String::new(),
                message_id: String::new(),
                outcome: crate::transports::SendOutcome::Failed,
                reason_code: Some("channel_full".to_string()),
                reason: Some("pty internal write channel full or closed".to_string()),
                details: None,
            });
        }
        outcome_rx
    }

    fn health(&self) -> TransportHealth {
        // The child exiting is the unreachable case: a pty whose process is gone
        // has no target left, and unlike ACP there is no respawn monitor that
        // might bring one back. A live child that is merely `Busy` or
        // `Initializing` is healthy — those belong to the readiness axis.
        let worker_live = self
            .worker_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished());
        let reader_live = self
            .reader_handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished());
        let reachable = self.write_tx.is_some()
            && worker_live
            && reader_live
            && !self.shared.child_exited.load(Ordering::Acquire);
        self.unreachable_since.fold(reachable)
    }

    async fn is_ready_for_handover(&self) -> bool {
        if self.write_tx.is_none()
            || self.shared.child_exited.load(Ordering::Acquire)
            || !matches!(self.readiness(), WorkerReadinessState::Available)
        {
            return false;
        }
        let mut probe = PtyPromptProbe::new(self.shared.clone());
        match tokio::time::timeout(PTY_PROBE_TIMEOUT, probe.observe()).await {
            Ok(Ok(ready)) => ready,
            Ok(Err(_)) => false,
            Err(_) => false,
        }
    }

    fn shutdown(&mut self) {
        // Mark the lifecycle as attempted so a subsequent
        // `startup()` hits the retry guard with `Unavailable` and
        // returns `Err(pty_unavailable_restart_unsupported)`.
        // Without this, a `shutdown()` call before any `startup()`
        // would leave `started == false`, and the next `startup()`
        // would proceed with init — only to have the worker exit
        // immediately because `shutdown_flag` is already true. The
        // retry guard catches this case; setting `started = true`
        // is what makes the guard observable.
        self.started = true;
        // Publish Unavailable FIRST so observers (look handle,
        // worker-state stream) see the transition before the worker
        // / reader threads drain and the child is killed. Mirrors
        // AcpTransport::shutdown's `set_readiness(Unavailable)`
        // ordering.
        self.set_readiness(WorkerReadinessState::Unavailable);
        self.shutdown_flag.store(true, Ordering::Release);
        self.write_tx = None;
        self.bytes_tx = None;
        // Kill the child FIRST so the PTY master closes; this wakes
        // the reader's blocking `read()`. The reader then sees
        // `Ok(0)` EOF, sets `child_exited=true`, and exits
        // naturally. We `wait()` the child BEFORE joining the
        // reader so the join order is deterministic and the
        // reader's EOF-driven exit happens on a closed master.
        if let Some(child_arc) = self.child.take()
            && let Ok(mut child) = child_arc.lock()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
    }

    fn give_output(&self) -> Option<Arc<dyn OutputView>> {
        if !self.has_live_runtime() {
            return None;
        }
        Some(Arc::new(PtyOutputView::new(self.shared.clone())))
    }
}

/// Inline write-seam test. `mailw`/`raww` on a full or closed write channel
/// must resolve the outcome immediately (`Failed` + `channel_full`) rather
/// than parking a delivery-runtime worker thread on `blocking_send` — the
/// relay documents that seam as non-blocking
/// (`src/relay/delivery/dispatch/worker.rs`), and the worker design rests on
/// that holding. The channel is a private transport field, so the full/closed
/// state can only be injected from inside the module; the block contains
/// exactly one `#[test]` function (per the project rule for inline private
/// tests), covering both seams against both channel states.
#[cfg(test)]
mod write_seam_tests {
    use super::*;
    use crate::configuration::{BundleMember, TargetConfiguration};
    use crate::envelope::AddressIdentity;
    use crate::transports::{DeliveryEnvelope, DeliveryMessage, SendOutcome};

    fn test_envelope(message_id: &str) -> DeliveryEnvelope {
        DeliveryEnvelope {
            message_id: message_id.to_string(),
            message: DeliveryMessage {
                body: "test body".to_string(),
                created_at: "2026-08-01T00:00:00Z".to_string(),
                namespace: "test-ns".to_string(),
                sender: AddressIdentity {
                    session_name: "sender@test-ns".to_string(),
                    display_name: None,
                },
                target: AddressIdentity {
                    session_name: "target@test-ns".to_string(),
                    display_name: None,
                },
                cc: Vec::new(),
                authenticated_identity: None,
                on_behalf_of: None,
            },
            append_enter: true,
            choice_decider_sessions: Vec::new(),
            is_receipt: false,
        }
    }

    fn test_transport() -> PtyTransport {
        PtyTransport::new(
            BundleMember {
                id: "test-session".to_string(),
                name: None,
                working_directory: None,
                target: TargetConfiguration::Ui,
                coder_session_id: None,
                policy_id: None,
                environment: Vec::new(),
            },
            PtyTargetConfiguration {
                initial_command: "/bin/sh".to_string(),
                resume_command: "/bin/sh".to_string(),
                prompt_readiness: None,
                cols: 120,
                rows: 40,
                working_directory: None,
                term_protocol: TermProtocol::default(),
            },
            None,
            // These fixtures never reach a write, so nothing is declared.
            // Refusing rather than accepting means a fixture that ever did reach
            // one would produce no effect and fail, where an accepting stub would
            // write against a unit the ledger never issued.
            Arc::new(NoDeclarations),
        )
    }

    struct NoDeclarations;
    impl PartitionSink for NoDeclarations {
        fn declare(
            &self,
            _member_ids: &[&str],
        ) -> Result<crate::transports::PackingUnitId, crate::transports::PartitionError> {
            Err(crate::transports::PartitionError::MemberNotBindable)
        }
        fn record(
            &self,
            _unit: crate::transports::PackingUnitId,
            _evidence: crate::transports::SubmissionEvidence,
        ) {
        }
    }

    /// Fill `capacity` slots of a fresh channel so the next `try_send` sees
    /// `Full`, then inject it as the transport's write channel.
    fn transport_with_full_channel() -> PtyTransport {
        let (write_tx, _write_rx) = mpsc::channel::<DeliveryCommand>(WRITE_CHANNEL_CAPACITY);
        for _ in 0..WRITE_CHANNEL_CAPACITY {
            let (outcome_tx, _outcome_rx) = oneshot::channel::<SingleDeliveryOutcome>();
            write_tx
                .blocking_send(DeliveryCommand::Raw {
                    content: String::new(),
                    append_enter: false,
                    outcome_tx,
                })
                .expect("fill write channel");
        }
        let mut transport = test_transport();
        transport.write_tx = Some(write_tx);
        transport
    }

    #[test]
    fn mailw_and_raww_resolve_immediately_when_the_channel_is_full_or_closed() {
        // Full channel: every slot is occupied, so `try_send` refuses without
        // blocking. Both seams must resolve the outcome immediately with
        // `Failed` + `channel_full`, never parking a delivery-runtime worker.
        let mut transport = transport_with_full_channel();

        let mailw_outcome = Transport::mailw(&mut transport, test_envelope("msg-1"))
            .blocking_recv()
            .expect("mailw must resolve immediately on a full channel");
        assert_eq!(mailw_outcome.outcome, SendOutcome::Failed);
        assert_eq!(
            mailw_outcome.reason_code.as_deref(),
            Some("channel_full"),
            "a full write channel must report channel_full, got {:?}",
            mailw_outcome.reason_code,
        );
        assert_eq!(mailw_outcome.message_id, "msg-1");

        let raww_outcome = Transport::raww(&mut transport, "raw text".to_string(), true)
            .blocking_recv()
            .expect("raww must resolve immediately on a full channel");
        assert_eq!(raww_outcome.outcome, SendOutcome::Failed);
        assert_eq!(raww_outcome.reason_code.as_deref(), Some("channel_full"));

        // Closed channel: the consumer is gone, so `try_send` refuses the item
        // back unchanged. Same immediate terminal resolution.
        let (write_tx, write_rx) = mpsc::channel::<DeliveryCommand>(WRITE_CHANNEL_CAPACITY);
        drop(write_rx);
        let mut transport = test_transport();
        transport.write_tx = Some(write_tx);

        let mailw_outcome = Transport::mailw(&mut transport, test_envelope("msg-2"))
            .blocking_recv()
            .expect("mailw must resolve immediately on a closed channel");
        assert_eq!(mailw_outcome.outcome, SendOutcome::Failed);
        assert_eq!(
            mailw_outcome.reason_code.as_deref(),
            Some("channel_full"),
            "a closed write channel must report channel_full, got {:?}",
            mailw_outcome.reason_code,
        );
        assert_eq!(mailw_outcome.message_id, "msg-2");

        let raww_outcome = Transport::raww(&mut transport, "raw text".to_string(), true)
            .blocking_recv()
            .expect("raww must resolve immediately on a closed channel");
        assert_eq!(raww_outcome.outcome, SendOutcome::Failed);
        assert_eq!(raww_outcome.reason_code.as_deref(), Some("channel_full"));
    }
}
