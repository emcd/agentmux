//! Pty transport core.
//!
//! [`PtyTransport`] is the per-target
//! [`Transport`](crate::transports::Transport) implementation. The
//! transport does NOT own the libghostty-vt terminal directly — the
//! terminal is constructed and lives entirely on the worker thread.
//!
//! Nothing is handed to this transport to deliver. Its worker thread runs the
//! shared delivery-loop executor, which asks the relay's mailbox what is waiting
//! for this target and writes it. The transport's own job is bring-up and
//! teardown of the thread that does so.
//!
//! Channels the transport owns:
//!
//! - `bytes_tx`: the reader thread feeds terminal output bytes into
//!   this channel; the worker feeds them into the terminal.
//!
//! Channels the worker thread owns:
//!
//! - `bytes_rx` (reader -> worker): terminal output bytes, fed to the
//!   terminal between executor iterations.
//! - `snapshot_rx` (look -> worker): routes snapshot requests to the thread
//!   that holds the terminal they read.
//!
//! The `libghostty-vt` terminal is `!Send + !Sync` (raw FFI pointers +
//! `dyn` trait object callbacks), so it must be constructed and live
//! entirely on the worker thread. The cross-thread coordination
//! between the relay's look path and the worker goes through the
//! snapshot channel; the cross-thread coordination between the reader
//! thread and the worker goes through the bytes channel.
//!
//! Per-coder config parsing (`[coders.<id>.pty]` → [`PtyTargetConfiguration`])
//! lands in §6.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use regex::Regex;
use tokio::sync::mpsc;

use crate::configuration::BundleMember;
use crate::configuration::TermProtocol;
use crate::transports::{
    DeliveryExecutorContext, GenerationFence, OutputView, StartupContext, Transport,
    TransportError, TransportHealth, TransportReadiness, TransportStatus, UnreachableSince,
    WorkerReadinessState,
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

use super::state::{PtyConfigSnapshot, PtyOutputView, PtyShared};

mod lifecycle;
mod runtime;

/// Default pty cols when the per-coder config does not set them.
pub const DEFAULT_COLS: u16 = 120;
/// Default pty rows when the per-coder config does not set them.
pub const DEFAULT_ROWS: u16 = 40;
/// Poll interval for the reader thread when the master returns
/// `WouldBlock`.
const READER_IDLE_POLL: Duration = Duration::from_millis(5);

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

/// Pty pane delivery transport, whose worker thread runs this target's one
/// serial delivery-loop executor.
///
/// That thread also processes PTY output and services snapshot requests, because
/// the terminal it owns cannot be moved off it.
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
    unreachable_since: Arc<UnreachableSince>,
    /// Live handle to the spawned child. `shutdown` kills and reaps.
    child: Option<Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>,
    /// Per-transport readiness state.
    readiness: Arc<Mutex<WorkerReadinessState>>,
    /// Optional relay-provided closure that mirrors per-turn readiness
    /// transitions into the relay's global worker-state registry.
    mirror_state: Option<PtyMirrorStateFn>,
    /// The mailbox handle, doorbell and policy the worker's executor runs
    /// against. Held here so `startup` can hand it to the thread it spawns.
    delivery: DeliveryExecutorContext,
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
        delivery: DeliveryExecutorContext,
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
            delivery,
            started: false,
            configured_initial_command: config.initial_command.clone(),
            configured_working_directory: config.working_directory.clone(),
            configured_term_protocol: config.term_protocol,
            bytes_tx: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            unreachable_since: Arc::new(UnreachableSince::default()),
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
    /// [`has_live_runtime`](Self::has_live_runtime) and by tests asserting
    /// lifecycle transitions.
    #[must_use]
    pub fn readiness(&self) -> WorkerReadinessState {
        *self.readiness.lock().expect("pty readiness mutex")
    }

    /// Whether the worker runtime exists and is usable, which `Busy` satisfies:
    /// a pty mid-turn still has a live master to snapshot.
    ///
    /// Deliberately not the write question. Whether the target can take a turn
    /// *now* is the executor's own, asked inside it against the terminal and
    /// excluding `Busy`; gating the output view on that stricter reading would
    /// withhold the `look` surface from exactly the target most worth looking at.
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
        let reachable =
            worker_live && reader_live && !self.shared.child_exited.load(Ordering::Acquire);
        self.unreachable_since.fold(reachable)
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
