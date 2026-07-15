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
//! libghostty-vt binding. The full delivery-task semantics (envelope
//! rendering, group coalesce, wedge/prime quiescence wait) lands in
//! §4.4 as a follow-up. Per-coder config parsing
//! (`[coders.<id>.pty]` → [`PtyTargetConfiguration`]) lands in §6.
//! The `TransportImpl::pty` cfg-gated wiring lands in §5.

use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use regex::Regex;
use tokio::sync::{mpsc, oneshot};

use crate::configuration::BundleMember;
use crate::configuration::TermProtocol;
use crate::transports::{
    DeliveryEnvelope, OutcomeFuture, OutputView, SingleDeliveryOutcome, StartupContext, Transport,
    TransportError, TransportReadiness, TransportStatus, WorkerReadinessState,
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

use super::command::program_and_args;
use super::state::{
    PtyConfigSnapshot, PtyOutputView, PtyShared, SnapshotRequest, SnapshotResponse,
};

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
/// Agentmux-pty version string returned by the `on_xtversion`
/// callback. The relay-tui can display it for the operator.
const PTY_VERSION_STRING: &str = concat!("agentmux-pty ", env!("CARGO_PKG_VERSION"));

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
    pub prime_timeout_ms: Option<u64>,
    pub wedge_detection: bool,
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
    /// successfully or with `Err`. Used by the `startup()` guard to
    /// distinguish never-attempted (`started == false`, the
    /// constructor state) from a re-attempt against an in-progress
    /// or previously-failed runtime. The readiness state alone
    /// cannot make this distinction: the constructor leaves
    /// readiness at `Initializing` (the same state a startup call
    /// that is currently in flight would expose), so a guard that
    /// rejects every `Initializing` would also reject the first
    /// legitimate call.
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
    /// Live handle to the spawned child. `shutdown` kills and reaps.
    child: Option<Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>,
    /// Per-transport readiness state. The relay thread reads it via
    /// `is_ready`; the worker thread mutates it via the cloned
    /// `Arc` after each lifecycle transition (Busy / Available /
    /// Unavailable). The relay-side guard `startup` consults the
    /// `Available` / `Busy` variants to skip re-init.
    readiness: Arc<Mutex<WorkerReadinessState>>,
    /// Optional relay-provided closure that mirrors per-turn readiness
    /// transitions into the relay's global worker-state registry. The
    /// relay dispatcher constructs it closing over
    /// `set_worker_readiness(namespace, runtime_directory,
    /// target_session, state)`; the transport holds an opaque
    /// `Arc<dyn Fn>` so `src/pty` does not import `crate::relay`.
    /// `None` in tests that construct the transport without a relay
    /// registry.
    mirror_state: Option<PtyMirrorStateFn>,
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
    /// per-coder configuration. Carries the configured
    /// initial/resume command, prompt-readiness template, grid dims,
    /// prime-timeout, wedge switch, and optional working directory
    /// into the shared `PtyConfigSnapshot` so the runtime probe
    /// (both cross-thread and worker-local) sees the same per-coder
    /// values.
    ///
    /// `mirror_state` is the relay-constructed closure that mirrors
    /// per-turn readiness transitions into the relay's global
    /// worker-state registry. Pass `None` in tests constructed
    /// without a relay registry; the transport's internal readiness
    /// state still advances so `is_ready()` can drive the lifecycle
    /// locally.
    #[must_use]
    pub fn new(
        target_member: BundleMember,
        config: PtyTargetConfiguration,
        mirror_state: Option<PtyMirrorStateFn>,
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
                prime_timeout_ms: config.prime_timeout_ms,
                wedge_detection: config.wedge_detection,
            },
            last_change_atomic: Arc::new(AtomicU64::new(0)),
            snapshot_tx: mpsc::channel(64).0,
            child_exited: Arc::new(AtomicBool::new(false)),
        };
        Self {
            target_member,
            shared,
            started: false,
            configured_initial_command: config.initial_command.clone(),
            configured_working_directory: config.working_directory.clone(),
            configured_term_protocol: config.term_protocol,
            write_tx: None,
            bytes_tx: None,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
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
        publish(&self.readiness, self.mirror_state.as_ref(), state);
    }

    /// Read the current readiness state. Used by `is_ready` and by
    /// tests asserting lifecycle transitions.
    #[must_use]
    pub fn readiness(&self) -> WorkerReadinessState {
        *self.readiness.lock().expect("pty readiness mutex")
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

    /// Performs the bootstrap work behind [`Transport::startup`].
    /// Opens the PTY pair, spawns the child, launches the worker and
    /// reader threads, and stashes the runtime handles. Separated from
    /// `startup` so the outer method can publish the
    /// `Initializing` → `Available | Unavailable` transitions uniformly
    /// on every code path (success, mapped errors, panicking spawns).
    ///
    /// On success, returns `Ok(TransportStatus::Ready)`. On any
    /// failure, the inner code path returns an `Err(TransportError)`
    /// WITHOUT calling `set_readiness` — the outer `startup` method
    /// publishes `Unavailable` after observing the error. On any
    /// failure after a child / worker / reader has been acquired, a
    /// [`StartupGuard`] holds the partial resources and tears them
    /// down on `Drop`. On success, [`StartupGuard::finish`] returns
    /// the owned resources and the caller moves them into `self`.
    fn startup_inner(
        &mut self,
        context: &StartupContext,
    ) -> Result<TransportStatus, TransportError> {
        let mut guard = StartupGuard::new(self.shutdown_flag.clone());

        let cols = self.shared.config.cols;
        let rows = self.shared.config.rows;
        // Use the per-coder initial_command set in the constructor (via
        // `PtyTargetConfiguration`). The bootstrap path carries the
        // resolved `[coders.<id>.pty].initial-command` (with
        // `{coder-session-id}` substitution) through the dispatcher;
        // when the dispatcher constructs the transport via
        // `TransportImpl::pty(target_member, config, mirror_state)` it
        // stores the resolved command here. Falls back to `/bin/bash`
        // for the tests' default-constructed transport.
        let initial_command = if self.configured_initial_command.is_empty() {
            "/bin/bash".to_string()
        } else {
            self.configured_initial_command.clone()
        };

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TransportError {
                code: "pty_open_failed".to_string(),
                reason: format!("portable_pty::openpty: {e}"),
                details: None,
            })?;

        // Tokenize the rendered command into program + args.
        // `CommandBuilder::new` takes a program path; passing the WHOLE
        // rendered string (e.g. "codex resume abc-123") would try to
        // exec a literal binary named "codex resume abc-123" and fail.
        // Shell-style tokenization handles quoting: "sh -lc 'exec
        // sleep 45'" tokenizes to ["sh", "-lc", "exec sleep 45"].
        let (program, args) = program_and_args(&initial_command).map_err(|e| TransportError {
            code: "pty_command_parse_failed".to_string(),
            reason: format!("tokenize initial_command {initial_command:?}: {e}"),
            details: None,
        })?;
        let mut cmd = CommandBuilder::new(&program);
        for arg in &args {
            cmd.arg(arg);
        }
        cmd.env("TERM", self.configured_term_protocol.as_env_var());
        cmd.env("COLORTERM", "truecolor");
        // Apply the merged coder/bundle/session environment last so an
        // operator-declared variable (including an explicit TERM/COLORTERM
        // override) wins over the transport defaults set above.
        for entry in &self.target_member.environment {
            cmd.env(entry.name.as_str(), entry.value.as_str());
        }
        // Prefer the per-coder `working_directory` (from
        // `[coders.<id>.pty]` or the bundle member's
        // `working_directory`); fall back to the runtime directory the
        // relay passes via `StartupContext::runtime_directory`.
        let cwd = self
            .configured_working_directory
            .as_deref()
            .or(Some(context.runtime_directory.as_path()));
        if let Some(cwd) = cwd.and_then(|p| p.to_str()) {
            cmd.cwd(cwd);
        }
        let child = pair.slave.spawn_command(cmd).map_err(|e| TransportError {
            code: "pty_spawn_failed".to_string(),
            reason: format!("spawn_command: {e}"),
            details: None,
        })?;
        drop(pair.slave);

        // Wrap the child handle in an `Arc<Mutex<...>>` and register
        // it with the guard IMMEDIATELY (before any fallible master
        // operation). A subsequent `try_clone_reader` / `take_writer`
        // failure then triggers `StartupGuard::Drop`, which kills +
        // waits the child before joining the threads. Without this,
        // a failure between `spawn_command` and the eventual
        // `self.child = Some(...)` assignment would leak the spawned
        // OS process — the local `child` value would fall out of
        // scope on `?` without ever being killed.
        let child_arc: PtyChildHandle = Arc::new(std::sync::Mutex::new(child));
        guard.note_child(child_arc.clone());

        let reader = pair.master.try_clone_reader().map_err(|e| TransportError {
            code: "pty_reader_clone_failed".to_string(),
            reason: format!("try_clone_reader: {e}"),
            details: None,
        })?;
        let writer = pair.master.take_writer().map_err(|e| TransportError {
            code: "pty_writer_take_failed".to_string(),
            reason: format!("take_writer: {e}"),
            details: None,
        })?;
        let writer_arc: Arc<std::sync::Mutex<Box<dyn Write + Send>>> =
            Arc::new(std::sync::Mutex::new(writer));

        let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>(256);
        let (write_tx, write_rx) = mpsc::channel::<DeliveryCommand>(WRITE_CHANNEL_CAPACITY);
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<SnapshotRequest>(64);

        self.shared.snapshot_tx = snapshot_tx.clone();
        let shared_for_worker = PtyShared {
            config: self.shared.config.clone(),
            last_change_atomic: self.shared.last_change_atomic.clone(),
            snapshot_tx,
            child_exited: self.shared.child_exited.clone(),
        };

        let target_session = self.target_member.id.clone();
        let target_session_for_worker = target_session.clone();
        let writer_for_worker = writer_arc.clone();
        let child_for_worker = child_arc.clone();
        let shutdown_flag_for_worker = self.shutdown_flag.clone();
        let mirror_state_for_worker = self.mirror_state.clone();
        let readiness_for_worker = self.readiness.clone();

        let bytes_tx_for_reader = bytes_tx.clone();
        let last_byte_atomic_for_reader = self.shared.last_change_atomic.clone();
        let reader_shutdown_flag = self.shutdown_flag.clone();
        let child_exited_for_reader = self.shared.child_exited.clone();

        let (init_tx, init_rx) = oneshot::channel::<Result<(), String>>();

        let worker_handle = thread::Builder::new()
            .name(format!("pty-worker-{target_session_for_worker}"))
            .spawn(move || {
                run_worker(
                    cols,
                    rows,
                    bytes_rx,
                    write_rx,
                    snapshot_rx,
                    shared_for_worker,
                    writer_for_worker,
                    child_for_worker,
                    target_session_for_worker,
                    shutdown_flag_for_worker,
                    Some(init_tx),
                    mirror_state_for_worker,
                    readiness_for_worker,
                );
            })
            .map_err(|e| TransportError {
                code: "pty_worker_spawn_failed".to_string(),
                reason: format!("worker thread spawn: {e}"),
                details: None,
            })?;
        guard.note_worker(worker_handle);

        let reader_handle = thread::Builder::new()
            .name(format!("pty-reader-{target_session}"))
            .spawn(move || {
                run_reader(
                    reader,
                    bytes_tx_for_reader,
                    last_byte_atomic_for_reader,
                    reader_shutdown_flag,
                    child_exited_for_reader,
                );
            })
            .map_err(|e| TransportError {
                code: "pty_reader_spawn_failed".to_string(),
                reason: format!("reader thread spawn: {e}"),
                details: None,
            })?;
        guard.note_reader(reader_handle);

        // Block on the worker's init-result handshake: the worker
        // publishes `WorkerReadinessState::Available` on success
        // BEFORE sending the init result, so when this recv returns
        // Ok(Ok(())) the local + relay-global readiness state is
        // already `Available`. On failure (Terminal::new error or
        // channel drop before init report) we surface the error and
        // the guard's Drop cleans up the partial runtime state.
        let result = match init_rx.blocking_recv() {
            Ok(Ok(())) => Ok(TransportStatus {
                readiness: TransportReadiness::Ready,
            }),
            Ok(Err(reason)) => Err(TransportError {
                code: "pty_init_failed".to_string(),
                reason,
                details: None,
            }),
            Err(_) => Err(TransportError {
                code: "pty_init_dropped".to_string(),
                reason: "pty worker thread exited before reporting initialization result"
                    .to_string(),
                details: None,
            }),
        };

        if result.is_ok() {
            // Success: disarm the guard and move the resources into
            // self. The guard's Drop is a no-op from here on; the
            // resources are owned by self.
            let (child, worker, reader) = guard.finish();
            self.write_tx = Some(write_tx);
            self.bytes_tx = Some(bytes_tx);
            self.child = Some(child);
            self.worker_handle = Some(worker);
            self.reader_handle = Some(reader);
        }
        // On Err, guard drops and tears down the partial state. The
        // `bytes_tx` / `write_tx` Senders fall out of scope here and
        // close their channels; the worker / reader threads see
        // their receivers return Err on the next poll and exit; the
        // child is killed + reaped by the guard.

        result
    }
}

/// Explicit ownership guard for partial startup resources. The
/// guard owns every acquired child / thread handle as soon as it
/// is registered, so a failure at any subsequent step triggers
/// `Drop`, which kills the child + joins both threads. On
/// success, [`StartupGuard::finish`] returns the owned resources
/// and the caller moves them into the parent `PtyTransport`.
/// `finish` sets the disarmed flag, so the subsequent `Drop` is
/// a no-op.
struct StartupGuard {
    shutdown_flag: Arc<AtomicBool>,
    child: Option<PtyChildHandle>,
    worker_handle: Option<thread::JoinHandle<()>>,
    reader_handle: Option<thread::JoinHandle<()>>,
    disarmed: bool,
}

type PtyChildHandle = Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>;

impl StartupGuard {
    fn new(shutdown_flag: Arc<AtomicBool>) -> Self {
        Self {
            shutdown_flag,
            child: None,
            worker_handle: None,
            reader_handle: None,
            disarmed: false,
        }
    }

    fn note_child(&mut self, child: PtyChildHandle) {
        self.child = Some(child);
    }

    fn note_worker(&mut self, handle: thread::JoinHandle<()>) {
        self.worker_handle = Some(handle);
    }

    fn note_reader(&mut self, handle: thread::JoinHandle<()>) {
        self.reader_handle = Some(handle);
    }

    /// Mark this guard as having successfully transferred its
    /// resources to the parent `PtyTransport`. Returns the owned
    /// resources; the subsequent `Drop` is a no-op because
    /// `disarmed == true` AND the resource fields are already
    /// `None` (consumed by this call).
    fn finish(
        mut self,
    ) -> (
        PtyChildHandle,
        thread::JoinHandle<()>,
        thread::JoinHandle<()>,
    ) {
        self.disarmed = true;
        let child = self
            .child
            .take()
            .expect("StartupGuard invariant: child is set before finish");
        let worker = self
            .worker_handle
            .take()
            .expect("StartupGuard invariant: worker_handle is set before finish");
        let reader = self
            .reader_handle
            .take()
            .expect("StartupGuard invariant: reader_handle is set before finish");
        (child, worker, reader)
    }
}

impl Drop for StartupGuard {
    fn drop(&mut self) {
        if self.disarmed {
            return;
        }
        // Best-effort partial cleanup. The order mirrors
        // `PtyTransport::shutdown`: kill the child FIRST so the
        // PTY master closes (this unblocks the reader's
        // `read()` via `Ok(0` EOF); the reader's `Err(_)` arm
        // also flips `child_exited`, but EOF is the
        // typical-path), then join the reader + worker.
        self.shutdown_flag.store(true, Ordering::Release);

        let child_arc = self.child.take();
        if let Some(child_arc) = child_arc {
            if let Ok(mut child) = child_arc.lock() {
                let _ = child.kill();
            }
            if let Ok(mut child) = child_arc.lock() {
                let _ = child.wait();
            }
        }

        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
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
        if write_tx.blocking_send(cmd).is_err() {
            // Channel closed; the consumer is gone. The OutcomeFuture
            // was returned to the caller; it will resolve to an error
            // because the consumer (worker thread) dropped the
            // outcome_tx when it shut down.
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
        if write_tx.blocking_send(cmd).is_err() {
            // Channel closed; the consumer is gone.
        }
        outcome_rx
    }

    fn is_ready(&self) -> bool {
        matches!(
            self.readiness(),
            WorkerReadinessState::Available | WorkerReadinessState::Busy
        )
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
        if !self.is_ready() {
            return None;
        }
        Some(Arc::new(PtyOutputView::new(self.shared.clone())))
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    cols: u16,
    rows: u16,
    mut bytes_rx: mpsc::Receiver<Vec<u8>>,
    mut write_rx: mpsc::Receiver<DeliveryCommand>,
    mut snapshot_rx: mpsc::Receiver<SnapshotRequest>,
    shared: PtyShared,
    writer: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    _child: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    target_session: String,
    shutdown_flag: Arc<AtomicBool>,
    // Init-result handshake: send `Ok(())` after a successful
    // Terminal::new + handler installation + initial `Available`
    // publish, or `Err(reason)` if Terminal::new failed. The relay's
    // `startup_inner` blocks on the receiving end and only returns
    // `TransportStatus::Ready` once `Ok(())` arrives, so a
    // `startup()`-reported `Ready` is guaranteed to post-date
    // the worker publishing `Available` (and `Ready` is reported
    // as a transport-level failure when the init result is
    // `Err`).
    init_tx: Option<oneshot::Sender<Result<(), String>>>,
    // Optional mirror closure for per-turn readiness transitions.
    // `None` in tests constructed without a relay registry; the
    // worker skips the publish in that case.
    mirror_state: Option<PtyMirrorStateFn>,
    // Cloned `Arc<Mutex<WorkerReadinessState>>` so the worker
    // thread can mutate the transport-local readiness state on
    // each lifecycle transition. Mirrors `AcpSharedState::readiness`
    // (see `src/acp/transport.rs`).
    readiness: Arc<Mutex<WorkerReadinessState>>,
) {
    // Construct the terminal INSIDE the worker thread. The terminal
    // is `!Send + !Sync` (raw FFI pointers inside); it cannot be moved
    // to this thread via `thread::spawn` so we build it here.
    let mut terminal = match libghostty_vt::Terminal::new(libghostty_vt::TerminalOptions {
        cols,
        rows,
        max_scrollback: 10_000,
    }) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[pty-worker-{target_session}] Terminal::new failed: {e}");
            let _ = init_tx.map(|tx| tx.send(Err(format!("Terminal::new failed: {e}"))));
            return;
        }
    };

    install_handlers(&mut terminal, writer.clone(), cols, rows);

    // Publish `Available` BEFORE signalling the init handshake so
    // that by the time `startup_inner` returns `TransportStatus::Ready`
    // the transport-local + relay-global readiness state is already
    // `Available`. The `startup` guard (which checks for `Available` /
    // `Busy`) sees a consistent post-init snapshot, and a caller
    // racing `is_ready` after `startup` returns sees `true` rather
    // than the brief `Initializing` window before the worker
    // publishes.
    publish(
        &readiness,
        mirror_state.as_ref(),
        WorkerReadinessState::Available,
    );
    let _ = init_tx.map(|tx| tx.send(Ok(())));

    // Event loop. Each iteration services snapshot_rx (user-facing
    // look requests) and the active delivery. When idle, it drains
    // bytes_rx (PTY output → terminal) and starts a new delivery
    // from write_rx.
    //
    // Critically, the worker does NOT block inside a delivery wait
    // — `Delivery::step` advances the state machine by ONE
    // classify-or-wait-poll per call. Between steps the worker
    // services other channels. The wait's `one_poll` drains
    // bytes_rx (so the terminal reflects the latest child output
    // before the next observation) and, for envelope-group waits,
    // absorbs write_rx envelopes. Raw-only waits do NOT drain
    // write_rx (avoids the v1 bug where raw waits absorbed
    // envelopes into a throwaway group and dropped them).
    //
    // Once the reader thread observes EOF on the PTY master
    // (`Ok(0`) or a fatal `Err(_)` (typical cause: the underlying
    // handle went bad on the OS side), it sets `child_exited=true`
    // and exits. The worker checks `child_exited` on every
    // iteration; once observed, it publishes `Unavailable`,
    // abandons the in-flight delivery (if any) with a
    // `pty_child_exited` `Failed` outcome, resolves the pending
    // raw (if any) with the same outcome, drains queued
    // `Envelope` / `Raw` commands with the same `Failed`
    // outcome, and breaks out of the loop. The worker MUST NOT
    // publish `Available` again until restart — the latched
    // condition keeps the local + relay-global readiness at
    // `Unavailable` even if a delivery would otherwise resolve
    // successfully. The `child_exited` flag lives on `PtyShared`
    // (the reader and worker both observe the same atomic), so the
    // latched condition is preserved across worker thread
    // lifetimes; `shutdown_flag` is not used for the latch because
    // `shutdown()` already publishes `Unavailable` and we want the
    // two paths to stay independent.
    let mut delivery: Option<super::delivery::Delivery> = None;
    let mut pending_raw: Option<super::delivery::PendingRaw> = None;
    let mut wait_in_progress = false;

    while !shutdown_flag.load(Ordering::Acquire) {
        // 0. Detect child-exit BEFORE servicing any other channel.
        // Once observed, publish `Unavailable`, abandon the
        // in-flight delivery (if any), resolve the pending raw
        // (if any), and drain queued commands with `Failed`
        // outcomes — then break out of the loop. The loop never
        // re-enters this branch because `child_exited` stays
        // `true` on `PtyShared` (no clearing — restart goes through
        // `Transport::startup`, which is currently blocked on
        // `Unavailable` until the respawn-monitor follow-up lands).
        if shared.child_exited.load(Ordering::Acquire) {
            publish(
                &readiness,
                mirror_state.as_ref(),
                WorkerReadinessState::Unavailable,
            );
            resolve_pending_raw_failed(&mut pending_raw, &target_session);
            abandon_in_flight(&mut delivery, &target_session);
            drain_remaining_with_failed(&mut write_rx, &target_session);
            break;
        }

        // 1. Always drain snapshot_rx. User-facing look requests
        // take priority over deliveries so the operator gets a
        // responsive view (the wait is decomposed into per-tick
        // polls so the worker loop services snapshot_rx between
        // wait ticks).
        while let Ok(request) = snapshot_rx.try_recv() {
            handle_snapshot(&mut terminal, request);
        }

        // 2. If a `Raw` is pending (from a prior delivery's batch
        // barrier, or a fresh raw from write_rx), start a new
        // raw-only delivery. The raw delivery's wait is
        // `WaitKind::RawOnly` — does not drain write_rx.
        if let Some(raw) = pending_raw.take() {
            match super::delivery::Delivery::start_raw(
                raw.content,
                raw.append_enter,
                raw.outcome_tx,
                &writer,
                &shared,
                &target_session,
            ) {
                Ok(d) => {
                    publish(
                        &readiness,
                        mirror_state.as_ref(),
                        WorkerReadinessState::Busy,
                    );
                    delivery = Some(d);
                    wait_in_progress = false;
                    continue;
                }
                Err(_) => {
                    // The failure outcome was already sent inside
                    // start_raw. Return to idle.
                    continue;
                }
            }
        }

        // 3. If a delivery is in progress, drive the state machine
        // one step. The step may resolve (Done), in which case we
        // drop the run and return to idle. A batch barrier during
        // the wait stashes the raw in `pending_raw` for the next
        // iteration.
        if let Some(d) = delivery.as_mut() {
            match d.step(
                &mut terminal,
                &mut bytes_rx,
                &mut write_rx,
                &shared,
                &target_session,
                &mut pending_raw,
            ) {
                super::delivery::DeliveryStep::Continue => {
                    // The wait is in progress. Sleep briefly to
                    // bound the wait-poll frequency; the outer
                    // loop will service snapshot_rx on the next
                    // iteration before the next step.
                    if wait_in_progress {
                        thread::sleep(super::delivery::WAIT_POLL_INTERVAL);
                    } else {
                        // First wait iteration just set up the
                        // wait; the next step will do the first
                        // poll. Brief sleep to avoid busy-looping.
                        thread::sleep(super::delivery::WAIT_POLL_INTERVAL);
                    }
                    wait_in_progress = true;
                    continue;
                }
                super::delivery::DeliveryStep::Done { wedged } => {
                    // Delivery resolved. Publish the readiness
                    // transition implied by the outcome kind:
                    // `Wedged` → `Unavailable` (per-turn failure
                    // observable to the relay); everything else
                    // (`Delivered` / `Timeout`) → `Available` (idle
                    // worker, awaiting the next write). The
                    // `child_exited` check on step 0 above means we
                    // can never reach this branch with the latch
                    // armed — the step-0 branch would have broken
                    // first.
                    let next = if wedged {
                        WorkerReadinessState::Unavailable
                    } else {
                        WorkerReadinessState::Available
                    };
                    publish(&readiness, mirror_state.as_ref(), next);
                    delivery = None;
                    wait_in_progress = false;
                    continue;
                }
            }
        }

        // 4. Idle: drain bytes_rx so the terminal reflects the
        // latest child output. This ensures look requests see fresh
        // data even when no delivery is in progress.
        while let Ok(bytes) = bytes_rx.try_recv() {
            terminal.vt_write(&bytes);
            shared.last_change_atomic.fetch_add(1, Ordering::AcqRel);
        }

        // 5. Try to start a new delivery from write_rx. An
        // `Envelope` starts a group delivery; a `Raw` is stashed
        // in `pending_raw` and processed by step (2) on the next
        // iteration.
        match write_rx.try_recv() {
            Ok(DeliveryCommand::Envelope {
                envelope,
                outcome_tx,
            }) => match super::delivery::Delivery::start_envelope_group(
                envelope,
                outcome_tx,
                &mut write_rx,
                &writer,
                &shared,
                &target_session,
            ) {
                Ok(d) => {
                    publish(
                        &readiness,
                        mirror_state.as_ref(),
                        WorkerReadinessState::Busy,
                    );
                    delivery = Some(d);
                    wait_in_progress = false;
                    continue;
                }
                Err(failure) => {
                    // Failure outcomes were already sent inside
                    // start_envelope_group; the worker just needs
                    // to honor the pending raw (if any).
                    pending_raw = failure.pending_raw;
                    continue;
                }
            },
            Ok(DeliveryCommand::Raw {
                content,
                append_enter,
                outcome_tx,
            }) => {
                pending_raw = Some(super::delivery::PendingRaw {
                    content,
                    append_enter,
                    outcome_tx,
                });
                continue;
            }
            Err(_) => {
                thread::sleep(WORKER_IDLE_POLL);
            }
        }
    }

    drain_remaining(&mut write_rx, &target_session);
    while let Ok(req) = snapshot_rx.try_recv() {
        let _ = req.tx.send(SnapshotResponse {
            tail: String::new(),
            cursor_x: 0,
            cursor_y: 0,
            cursor_visible: false,
        });
    }
}

/// Abandon an in-flight delivery by resolving it as `Failed` with a
/// `pty_child_exited` reason. Called from the child-exit branch of the
/// worker event loop so the caller's `OutcomeFuture` does not hang
/// after the worker has observed child death and refused to drive
/// the delivery to its terminal classification. Best-effort: a
/// `send` failure (the caller has dropped their receiver) is silently
/// swallowed.
fn abandon_in_flight(delivery: &mut Option<super::delivery::Delivery>, target_session: &str) {
    if let Some(mut d) = delivery.take() {
        d.abandon_into_failed(
            target_session,
            "pty_child_exited",
            "pty child exited before delivery resolved",
        );
    }
}

/// Drain any remaining queued `Envelope`/`Raw` commands from the
/// relay's write channel after the worker has observed child exit.
/// Each `outcome_tx` is resolved with `Failed` /
/// `reason_code = "pty_child_exited"` so the relay's collected
/// `OutcomeFuture`s do not hang on dropped senders.
fn drain_remaining_with_failed(
    write_rx: &mut mpsc::Receiver<DeliveryCommand>,
    target_session: &str,
) {
    while let Ok(cmd) = write_rx.try_recv() {
        let outcome_tx = match cmd {
            DeliveryCommand::Envelope { outcome_tx, .. } => outcome_tx,
            DeliveryCommand::Raw { outcome_tx, .. } => outcome_tx,
        };
        let _ = outcome_tx.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_child_exited".to_string()),
            reason: Some(
                "pty child exited before the queued delivery could be processed".to_string(),
            ),
            details: None,
        });
    }
}

/// Resolve a `PendingRaw` (a queued `Raw` command that has been
/// parsed off `write_rx` but not yet handed to
/// `Delivery::start_raw`) as `Failed` /
/// `reason_code = "pty_child_exited"`. Called from the child-exit
/// branch of the worker event loop alongside `abandon_in_flight`
/// and `drain_remaining_with_failed` so that the raw's
/// `outcome_tx` is resolved deterministically — without this, the
/// raw's caller would receive a closed-channel error after the
/// worker thread exits, not the `pty_child_exited` failure the
/// caller can correlate with the same `reason_code` the other
/// queued + in-flight deliveries received.
fn resolve_pending_raw_failed(
    pending_raw: &mut Option<super::delivery::PendingRaw>,
    target_session: &str,
) {
    if let Some(raw) = pending_raw.take() {
        let _ = raw.outcome_tx.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_child_exited".to_string()),
            reason: Some("pty child exited before the pending raw could be processed".to_string()),
            details: None,
        });
    }
}

/// Update the transport-local readiness mutex AND (when injected)
/// mirror the transition into the relay's global worker-state
/// registry. Single source of truth for every readiness publish on
/// the worker thread.
fn publish(
    readiness: &Arc<Mutex<WorkerReadinessState>>,
    mirror_state: Option<&PtyMirrorStateFn>,
    state: WorkerReadinessState,
) {
    *readiness.lock().expect("pty readiness mutex") = state;
    if let Some(mirror) = mirror_state {
        mirror(state);
    }
}

/// Install the libghostty-vt effect handlers on the worker-thread
/// terminal. The handlers run synchronously on the worker thread (the
/// only thread that owns the terminal). They close over the writer
/// Arc + cols/rows captures so responses flow back to the PTY master.
fn install_handlers(
    terminal: &mut libghostty_vt::Terminal<'_, '_>,
    writer: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    cols: u16,
    rows: u16,
) {
    let writer_for_pty_write = writer.clone();
    terminal
        .on_pty_write(move |_t, data| {
            if let Ok(mut g) = writer_for_pty_write.lock() {
                let _ = g.write_all(data);
            }
        })
        .expect("install on_pty_write callback");
    let _ = (writer, cols, rows); // Future handlers will capture these; reserved.
    terminal
        .on_size(move |_t| {
            Some(libghostty_vt::terminal::SizeReportSize {
                rows,
                columns: cols,
                cell_width: 8,
                cell_height: 16,
            })
        })
        .expect("install on_size callback");
    terminal
        .on_device_attributes(|_t| {
            use libghostty_vt::terminal::{
                ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType,
                PrimaryDeviceAttributes, SecondaryDeviceAttributes,
            };
            Some(DeviceAttributes {
                primary: PrimaryDeviceAttributes::new(
                    ConformanceLevel::VT220,
                    &[
                        DeviceAttributeFeature::COLUMNS_132,
                        DeviceAttributeFeature::SELECTIVE_ERASE,
                        DeviceAttributeFeature::ANSI_COLOR,
                    ],
                ),
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType::VT220,
                    firmware_version: 1,
                    rom_cartridge: 0,
                },
                tertiary: Default::default(),
            })
        })
        .expect("install on_device_attributes callback");
    terminal
        .on_xtversion(|_t| Some(PTY_VERSION_STRING))
        .expect("install on_xtversion callback");
    terminal
        .on_title_changed(|_t| {
            // Title stream events land in §8 worker readiness; for
            // v1 we do nothing. The terminal's title is preserved in
            // its internal state and surfaces in PtyOutputView
            // snapshots.
        })
        .expect("install on_title_changed callback");
}

fn handle_snapshot(terminal: &mut libghostty_vt::Terminal<'_, '_>, request: SnapshotRequest) {
    let response = render_snapshot(terminal, request.inspect_lines);
    let _ = request.tx.send(response);
}

fn render_snapshot(
    terminal: &mut libghostty_vt::Terminal<'_, '_>,
    inspect_lines: Option<usize>,
) -> SnapshotResponse {
    // The formatter borrows the terminal immutably; we drop the
    // formatter before any other terminal mutation.
    let formatter_result = libghostty_vt::fmt::Formatter::new(
        terminal,
        libghostty_vt::fmt::FormatterOptions::new()
            .with_format(libghostty_vt::fmt::Format::Plain)
            .with_trim(true),
    );
    let bytes = match formatter_result {
        Ok(formatter) => {
            let mut f = formatter;
            match f.format_alloc(None) {
                Ok(bytes) => bytes.as_ref().to_vec(),
                Err(_) => Vec::new(),
            }
        }
        Err(_) => Vec::new(),
    };
    let tail = String::from_utf8_lossy(&bytes).to_string();
    let lines_to_take = inspect_lines.unwrap_or(super::state::LOOK_LINES_DEFAULT);
    let mut collected: Vec<String> = tail
        .lines()
        .rev()
        .take(lines_to_take)
        .map(str::to_string)
        .collect();
    collected.reverse();
    let trimmed_tail = collected.join("\n");
    let cursor_x = terminal.cursor_x().unwrap_or(0);
    let cursor_y = terminal.cursor_y().unwrap_or(0);
    let cursor_visible = terminal.is_cursor_visible().unwrap_or(false);
    SnapshotResponse {
        tail: trimmed_tail,
        cursor_x,
        cursor_y,
        cursor_visible,
    }
}

fn drain_remaining(write_rx: &mut mpsc::Receiver<DeliveryCommand>, _target_session: &str) {
    while write_rx.try_recv().is_ok() {}
}

fn run_reader(
    mut reader: Box<dyn std::io::Read + Send>,
    bytes_tx: mpsc::Sender<Vec<u8>>,
    _last_change_atomic: Arc<AtomicU64>,
    shutdown_flag: Arc<AtomicBool>,
    child_exited: Arc<AtomicBool>,
) {
    // The reader forwards raw bytes to the worker. The change atomic
    // is updated by the worker AFTER `terminal.vt_write(&bytes)` so
    // the quiescence probe sees a generation advance only when the
    // terminal has actually consumed the bytes. Updating the atomic
    // here (before vt_write) would let `wait_for_change` return while
    // the terminal still shows the old screen, producing stale
    // readiness decisions.
    //
    // On EOF (`Ok(0)` — the master has closed, typically because
    // the spawned child exited or the relay called `shutdown()`'s
    // `child.kill`/`wait`), set `child_exited` so the worker thread
    // observes the death on its next loop iteration. The worker
    // latches the condition and refuses to publish `Available`
    // again (see `run_worker`'s terminal-state branch).
    //
    // On a fatal `Err(_)` (anything other than `WouldBlock`), treat
    // the read failure as transport death: the PTY master has
    // become unusable (typical cause: the underlying terminal
    // handle went bad on the OS side). Set `child_exited` and exit
    // the same way we do on `Ok(0)`. Without this, the worker
    // would never observe transport death and would sit indefinitely
    // in a `Busy`/`Available` state while the child is effectively
    // dead.
    let mut buf = vec![0u8; 4096];
    while !shutdown_flag.load(Ordering::Acquire) {
        match reader.read(&mut buf) {
            Ok(0) => {
                child_exited.store(true, Ordering::Release);
                break;
            }
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                if bytes_tx.blocking_send(bytes).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(READER_IDLE_POLL);
            }
            Err(_) => {
                // Fatal read error (anything other than WouldBlock):
                // the PTY master has become unusable (typical cause:
                // the underlying handle went bad on the OS side, or
                // the master was closed from the outside). Treat as
                // transport death: set `child_exited` so the worker
                // thread observes the latch on its next loop
                // iteration. Without this, the worker would sit
                // indefinitely in `Busy` / `Available` while the
                // child is effectively dead.
                child_exited.store(true, Ordering::Release);
                break;
            }
        }
    }
}

impl PtyConfigSnapshot {
    /// Re-export of `target_member_id` for callers that need a stable
    /// identifier in the worker thread's logs and diagnostic payloads.
    pub fn target_id(&self) -> &str {
        &self.target_member_id
    }
}

/// Inline private test block. Covers the two private-only
/// branches that have no public exerciser path: the fatal
/// non-`WouldBlock` `Err` arm of `run_reader`'s `read()` loop
/// (which sets `child_exited`), and the `resolve_pending_raw_failed`
/// helper (which sends `Failed` + `pty_child_exited` on the
/// pending raw's `outcome_tx`). Both helpers are crate-private by
/// design; widening their visibility solely for tests would
/// introduce escape hatches into the production API. The block
/// contains exactly one `#[test]` function (per the project rule
/// for inline private tests).
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Read};

    /// `Read` that returns a fatal non-`WouldBlock` error on every
    /// `read()` call. Used to drive `run_reader`'s `Err(_)` arm
    /// (which sets `child_exited`).
    struct FatalErrReader;

    impl Read for FatalErrReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("simulated fatal read"))
        }
    }

    #[test]
    fn lifecycle_internals_compose_correctly() {
        // Branch 1: `run_reader` with a fatal non-`WouldBlock` Err
        // sets `child_exited` so the worker's child-exit branch
        // fires regardless of whether the master closed via EOF or
        // via a fatal read error.
        let child_exited = Arc::new(AtomicBool::new(false));
        let (bytes_tx, _bytes_rx) = mpsc::channel::<Vec<u8>>(256);
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        run_reader(
            Box::new(FatalErrReader),
            bytes_tx,
            Arc::new(AtomicU64::new(0)),
            shutdown_flag,
            child_exited.clone(),
        );
        assert!(
            child_exited.load(Ordering::Acquire),
            "fatal reader Err (non-WouldBlock) should set child_exited",
        );

        // Branch 2: `resolve_pending_raw_failed` sends
        // `Failed` + `pty_child_exited` on the pending raw's
        // `outcome_tx` and consumes the slot. The pending raw exists
        // only briefly between the batch-barrier poll and the next
        // worker iteration; the group may resolve `raw_interrupted`
        // and the raw may start before child exit, making the
        // end-to-end timing inherently nondeterministic. This
        // deterministic helper-level test is the proof for the
        // private slot's contract.
        let (tx, rx) = oneshot::channel::<SingleDeliveryOutcome>();
        let mut pending_raw = Some(super::super::delivery::PendingRaw {
            content: "x".to_string(),
            append_enter: false,
            outcome_tx: tx,
        });
        resolve_pending_raw_failed(&mut pending_raw, "test-session");
        assert!(
            pending_raw.is_none(),
            "pending_raw slot should be consumed by resolve_pending_raw_failed",
        );
        let outcome = rx
            .blocking_recv()
            .expect("receiver should get the Failed outcome");
        assert_eq!(outcome.target_session, "test-session");
        assert!(
            matches!(outcome.outcome, crate::transports::SendOutcome::Failed),
            "expected Failed, got {:?}",
            outcome.outcome,
        );
        assert_eq!(
            outcome.reason_code.as_deref(),
            Some("pty_child_exited"),
            "expected reason_code pty_child_exited, got {:?}",
            outcome.reason_code,
        );
    }
}
