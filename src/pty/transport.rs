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
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use regex::Regex;
use tokio::sync::{mpsc, oneshot};

use crate::configuration::BundleMember;
use crate::transports::{
    DeliveryEnvelope, OutcomeFuture, OutputView, SingleDeliveryOutcome, StartupContext, Transport,
    TransportError, TransportReadiness, TransportStatus, wait_for_quiescent_three_state,
};

use super::state::{
    PtyConfigSnapshot, PtyOutputView, PtyShared, SnapshotRequest, SnapshotResponse,
    WorkerTerminalProbe,
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
/// Default quiet window for the wedge / prime quiescence wait.
const QUIET_WINDOW: Duration = Duration::from_millis(50);
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
    /// Write-command channel the relay submits into. `None` before
    /// `startup`; `Some` once the worker thread is running.
    write_tx: Option<mpsc::Sender<DeliveryCommand>>,
    /// Bytes channel the reader thread feeds into the worker.
    /// `None` before `startup`; `Some` once the worker thread is running.
    bytes_tx: Option<mpsc::Sender<Vec<u8>>>,
    /// Set to `true` once `startup` completes successfully.
    ready: Arc<AtomicBool>,
    /// Set to `true` by `shutdown` so the worker / reader threads
    /// drain and exit cleanly.
    shutdown_flag: Arc<AtomicBool>,
    /// Handle to the worker thread. Joined by `shutdown`.
    worker_handle: Option<thread::JoinHandle<()>>,
    /// Handle to the reader thread. Joined by `shutdown`.
    reader_handle: Option<thread::JoinHandle<()>>,
    /// Live handle to the spawned child. `shutdown` kills and reaps.
    child: Option<Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>>,
}

impl std::fmt::Debug for PtyTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PtyTransport")
            .field("target_member", &self.target_member)
            .field("ready", &self.ready.load(Ordering::Acquire))
            .finish()
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
    #[must_use]
    pub fn new(target_member: BundleMember, config: PtyTargetConfiguration) -> Self {
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
        };
        Self {
            target_member,
            shared,
            configured_initial_command: config.initial_command.clone(),
            configured_working_directory: config.working_directory.clone(),
            write_tx: None,
            bytes_tx: None,
            ready: Arc::new(AtomicBool::new(false)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            worker_handle: None,
            reader_handle: None,
            child: None,
        }
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

impl Transport for PtyTransport {
    fn startup(&mut self, context: StartupContext) -> Result<TransportStatus, TransportError> {
        if self.ready.load(Ordering::Acquire) {
            return Ok(TransportStatus {
                readiness: TransportReadiness::Ready,
            });
        }

        let cols = self.shared.config.cols;
        let rows = self.shared.config.rows;
        // Use the per-coder initial_command set in the constructor (via
        // `PtyTargetConfiguration`). The bootstrap path carries the
        // resolved `[coders.<id>.pty].initial-command` (with
        // `{coder-session-id}` substitution) through the dispatcher;
        // when the dispatcher constructs the transport via
        // `TransportImpl::pty(target_member, config)` it stores the
        // resolved command here. Falls back to `/bin/bash` for the
        // tests' default-constructed transport.
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

        let mut cmd = CommandBuilder::new(&initial_command);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
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

        let child_arc: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>> =
            Arc::new(std::sync::Mutex::new(child));

        let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>(256);
        let (write_tx, write_rx) = mpsc::channel::<DeliveryCommand>(WRITE_CHANNEL_CAPACITY);
        let (snapshot_tx, snapshot_rx) = mpsc::channel::<SnapshotRequest>(64);

        self.shared.snapshot_tx = snapshot_tx.clone();
        let shared_for_worker = PtyShared {
            config: self.shared.config.clone(),
            last_change_atomic: self.shared.last_change_atomic.clone(),
            snapshot_tx,
        };

        let target_session = self.target_member.id.clone();
        let target_session_for_worker = target_session.clone();
        let writer_for_worker = writer_arc.clone();
        let child_for_worker = child_arc.clone();
        let shutdown_flag_for_worker = self.shutdown_flag.clone();

        let bytes_tx_for_reader = bytes_tx.clone();
        let last_byte_atomic_for_reader = self.shared.last_change_atomic.clone();
        let reader_shutdown_flag = self.shutdown_flag.clone();

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
                );
            })
            .map_err(|e| TransportError {
                code: "pty_worker_spawn_failed".to_string(),
                reason: format!("worker thread spawn: {e}"),
                details: None,
            })?;

        let reader_handle = thread::Builder::new()
            .name(format!("pty-reader-{target_session}"))
            .spawn(move || {
                run_reader(
                    reader,
                    bytes_tx_for_reader,
                    last_byte_atomic_for_reader,
                    reader_shutdown_flag,
                );
            })
            .map_err(|e| TransportError {
                code: "pty_reader_spawn_failed".to_string(),
                reason: format!("reader thread spawn: {e}"),
                details: None,
            })?;

        self.write_tx = Some(write_tx);
        self.bytes_tx = Some(bytes_tx);
        self.child = Some(child_arc);
        self.worker_handle = Some(worker_handle);
        self.reader_handle = Some(reader_handle);
        self.ready.store(true, Ordering::Release);

        Ok(TransportStatus {
            readiness: TransportReadiness::Ready,
        })
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
        self.ready.load(Ordering::Acquire)
    }

    fn shutdown(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        self.write_tx = None;
        self.bytes_tx = None;
        if let Some(handle) = self.worker_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }
        if let Some(child_arc) = self.child.take()
            && let Ok(mut child) = child_arc.lock()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.ready.store(false, Ordering::Release);
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
            return;
        }
    };

    install_handlers(&mut terminal, writer.clone(), cols, rows);

    while !shutdown_flag.load(Ordering::Acquire) {
        // Priority: snapshot > write > bytes. Snapshots are
        // user-facing and should be quick; writes advance delivery;
        // bytes are continuous terminal output. Coalesce-during-wait
        // semantics (§4.4): absorb all available envelopes from the
        // write channel before quiescing; raw items are batch barriers
        // (the group is flushed and pasted before the raw write).
        if let Ok(request) = snapshot_rx.try_recv() {
            handle_snapshot(&mut terminal, request);
            continue;
        }
        if let Ok(first_cmd) = write_rx.try_recv() {
            flush_delivery_group(
                &mut terminal,
                first_cmd,
                &mut write_rx,
                &writer,
                &shared,
                &target_session,
            );
            continue;
        }
        if let Ok(bytes) = bytes_rx.try_recv() {
            terminal.vt_write(&bytes);
            // Advance the change atomic AFTER vt_write so the
            // quiescence probe sees a generation advance only when
            // the terminal has actually consumed the bytes.
            shared.last_change_atomic.fetch_add(1, Ordering::AcqRel);
            continue;
        }
        thread::sleep(WORKER_IDLE_POLL);
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

#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
fn handle_delivery_command(
    terminal: &mut libghostty_vt::Terminal<'static, 'static>,
    cmd: DeliveryCommand,
    writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    shared: &PtyShared,
    target_session: &str,
    write_rx: &mut mpsc::Receiver<DeliveryCommand>,
) {
    // Delegates to `flush_delivery_group` which absorbs further envelopes
    // into the same group. The single-cmd noop shims `handle_envelope` /
    // `handle_raw` are kept for callers that might submit a lone command.
    flush_delivery_group(terminal, cmd, write_rx, writer, shared, target_session);
}

/// Coalesce-during-wait delivery for one initial `DeliveryCommand`.
///
/// Absorbs additional envelopes that arrive during the quiescence wait
/// into the current group (matching the Tmux flush_and_resolve
/// semantics). A `Raw` command acts as a batch barrier: the
/// accumulated envelope group is flushed first, then the raw bytes
/// are written. The quiescence wait uses [`WorkerTerminalProbe`]
/// so the worker observes the terminal directly (routing through the
/// snapshot channel would self-deadlock the worker, since the same
/// worker thread is both the receiver and the caller).
#[allow(clippy::too_many_arguments)]
fn flush_delivery_group(
    terminal: &mut libghostty_vt::Terminal<'static, 'static>,
    first_cmd: DeliveryCommand,
    write_rx: &mut mpsc::Receiver<DeliveryCommand>,
    writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    shared: &PtyShared,
    target_session: &str,
) {
    let mut group: Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )> = Vec::new();
    match first_cmd {
        DeliveryCommand::Envelope {
            envelope,
            outcome_tx,
        } => group.push((envelope, outcome_tx)),
        DeliveryCommand::Raw {
            content,
            append_enter,
            outcome_tx,
        } => {
            handle_raw(
                terminal,
                content,
                append_enter,
                writer,
                shared,
                target_session,
                outcome_tx,
            );
            return;
        }
    }
    loop {
        match write_rx.try_recv() {
            Ok(DeliveryCommand::Envelope {
                envelope,
                outcome_tx,
            }) => group.push((envelope, outcome_tx)),
            Ok(DeliveryCommand::Raw {
                content,
                append_enter,
                outcome_tx,
            }) => {
                if !group.is_empty() {
                    paste_envelope_group(terminal, &mut group, writer, shared, target_session);
                }
                handle_raw(
                    terminal,
                    content,
                    append_enter,
                    writer,
                    shared,
                    target_session,
                    outcome_tx,
                );
                return;
            }
            Err(_) => break,
        }
    }
    paste_envelope_group(terminal, &mut group, writer, shared, target_session);
}

fn envelope_outcome_from_wait_result(
    wait_result: Result<String, crate::transports::DeliveryWaitError>,
    target_session: &str,
) -> SingleDeliveryOutcome {
    match wait_result {
        Ok(_pane_target) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Delivered,
            reason_code: None,
            reason: None,
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Timeout {
            timeout,
            readiness_mismatch,
            mismatch_reason,
        }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Timeout,
            reason_code: Some("delivery_prime_timeout".to_string()),
            reason: Some(format!(
                "prime wait timed out after {}ms (readiness_mismatch={}, reason={:?})",
                timeout.as_millis(),
                readiness_mismatch,
                mismatch_reason
            )),
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Wedged { reason }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pane_wedged".to_string()),
            reason: Some(format!("pty pane wedged: {reason}")),
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Failed { reason }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_probe_failed".to_string()),
            reason: Some(reason),
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Shutdown) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::DroppedOnShutdown,
            reason_code: Some("dropped_on_shutdown".to_string()),
            reason: Some("delivery dropped due to relay shutdown".to_string()),
            details: None,
        },
    }
}

fn paste_envelope_group(
    terminal: &mut libghostty_vt::Terminal<'static, 'static>,
    group: &mut Vec<(
        Box<DeliveryEnvelope>,
        oneshot::Sender<SingleDeliveryOutcome>,
    )>,
    writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    shared: &PtyShared,
    target_session: &str,
) {
    if group.is_empty() {
        return;
    }
    for (envelope, _) in group.iter() {
        let text = envelope.message.render_pane_envelope(&envelope.message_id);
        if let Err(e) = (|| -> std::io::Result<()> {
            let mut g = writer
                .lock()
                .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
            g.write_all(text.as_bytes())?;
            g.write_all(b"\n")?;
            Ok(())
        })() {
            let reason = format!("pty master write: {e}");
            for (env, sender) in group.drain(..) {
                let _ = sender.send(SingleDeliveryOutcome {
                    target_session: target_session.to_string(),
                    message_id: env.message_id,
                    outcome: crate::transports::SendOutcome::Failed,
                    reason_code: Some("pty_write_failed".to_string()),
                    reason: Some(reason.clone()),
                    details: None,
                });
            }
            return;
        }
    }
    let probe = WorkerTerminalProbe::new(
        terminal,
        shared.config.clone(),
        shared.last_change_atomic.clone(),
    );
    let prime_started_at = Instant::now();
    let prime_timeout_ms = shared.config.prime_timeout_ms;
    let prime_deadline = prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));
    let wedge_detection = shared.config.wedge_detection;
    let wait_result = wait_for_quiescent_three_state(
        &mut { probe },
        target_session,
        QUIET_WINDOW,
        prime_deadline,
        prime_started_at,
        prime_timeout_ms,
        wedge_detection,
    );
    let base_outcome = envelope_outcome_from_wait_result(wait_result, target_session);
    for (env, sender) in group.drain(..) {
        let mut env_outcome = base_outcome.clone();
        env_outcome.message_id = env.message_id;
        let _ = sender.send(env_outcome);
    }
}

#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
fn handle_envelope(
    terminal: &mut libghostty_vt::Terminal<'static, 'static>,
    envelope: Box<DeliveryEnvelope>,
    writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    shared: &PtyShared,
    target_session: &str,
    outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
) {
    // Single-envelope delivery shim. The production path coalesces
    // envelopes via `flush_delivery_group`, which calls
    // `paste_envelope_group` directly. This shim is kept for callers
    // that might submit a single-envelope command and is also the test
    // seam for the unit tests.
    let mut group = vec![(envelope, outcome_tx)];
    paste_envelope_group(terminal, &mut group, writer, shared, target_session);
}

#[allow(clippy::too_many_arguments)]
fn handle_raw(
    terminal: &mut libghostty_vt::Terminal<'static, 'static>,
    content: String,
    append_enter: bool,
    writer: &Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    shared: &PtyShared,
    target_session: &str,
    outcome_tx: oneshot::Sender<SingleDeliveryOutcome>,
) {
    let write_result = (|| -> std::io::Result<()> {
        let mut g = writer
            .lock()
            .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
        g.write_all(content.as_bytes())?;
        if append_enter {
            g.write_all(b"\n")?;
        }
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = outcome_tx.send(SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_write_failed".to_string()),
            reason: Some(format!("pty master write: {e}")),
            details: None,
        });
        return;
    }
    let probe = WorkerTerminalProbe::new(
        terminal,
        shared.config.clone(),
        shared.last_change_atomic.clone(),
    );
    let prime_started_at = Instant::now();
    let prime_timeout_ms = shared.config.prime_timeout_ms;
    let prime_deadline = prime_timeout_ms.map(|ms| prime_started_at + Duration::from_millis(ms));
    let wedge_detection = shared.config.wedge_detection;
    let wait_result = wait_for_quiescent_three_state(
        &mut { probe },
        target_session,
        QUIET_WINDOW,
        prime_deadline,
        prime_started_at,
        prime_timeout_ms,
        wedge_detection,
    );
    let outcome = match wait_result {
        Ok(_pane_target) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Delivered,
            reason_code: None,
            reason: None,
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Timeout {
            timeout,
            readiness_mismatch,
            mismatch_reason,
        }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Timeout,
            reason_code: Some("delivery_prime_timeout".to_string()),
            reason: Some(format!(
                "prime wait timed out after {}ms (readiness_mismatch={}, reason={:?})",
                timeout.as_millis(),
                readiness_mismatch,
                mismatch_reason
            )),
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Wedged { reason }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pane_wedged".to_string()),
            reason: Some(format!("pty pane wedged: {reason}")),
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Failed { reason }) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::Failed,
            reason_code: Some("pty_probe_failed".to_string()),
            reason: Some(reason),
            details: None,
        },
        Err(crate::transports::DeliveryWaitError::Shutdown) => SingleDeliveryOutcome {
            target_session: target_session.to_string(),
            message_id: String::new(),
            outcome: crate::transports::SendOutcome::DroppedOnShutdown,
            reason_code: Some("dropped_on_shutdown".to_string()),
            reason: Some("delivery dropped due to relay shutdown".to_string()),
            details: None,
        },
    };
    let _ = outcome_tx.send(outcome);
}

fn drain_remaining(write_rx: &mut mpsc::Receiver<DeliveryCommand>, _target_session: &str) {
    while write_rx.try_recv().is_ok() {}
}

fn run_reader(
    mut reader: Box<dyn std::io::Read + Send>,
    bytes_tx: mpsc::Sender<Vec<u8>>,
    _last_change_atomic: Arc<AtomicU64>,
    shutdown_flag: Arc<AtomicBool>,
) {
    // The reader forwards raw bytes to the worker. The change atomic
    // is updated by the worker AFTER `terminal.vt_write(&bytes)` so
    // the quiescence probe sees a generation advance only when the
    // terminal has actually consumed the bytes. Updating the atomic
    // here (before vt_write) would let `wait_for_change` return while
    // the terminal still shows the old screen, producing stale
    // readiness decisions.
    let mut buf = vec![0u8; 4096];
    while !shutdown_flag.load(Ordering::Acquire) {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                if bytes_tx.blocking_send(bytes).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(READER_IDLE_POLL);
            }
            Err(_) => break,
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
