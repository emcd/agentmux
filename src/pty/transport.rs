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
//! - `bytes_tx`: the worker re-emits the terminal snapshot requests
//!   into this channel when the relay's look path asks for them.
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
    time::Duration,
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::{mpsc, oneshot};

use crate::configuration::BundleMember;
use crate::transports::{
    DeliveryEnvelope, OutcomeFuture, OutputView, SingleDeliveryOutcome, StartupContext, Transport,
    TransportError, TransportReadiness, TransportStatus,
};

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

/// Per-coder pty target configuration as parsed from
/// `[coders.<id>.pty]`. The full config surface lands in §6.
#[derive(Clone, Debug)]
pub struct PtyTargetConfiguration {
    pub initial_command: String,
    pub resume_command: String,
    pub cols: u16,
    pub rows: u16,
    pub prime_timeout_ms: Option<u64>,
    pub wedge_detection: bool,
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
    /// per-coder configuration.
    #[must_use]
    pub fn new(target_member: BundleMember, config: PtyTargetConfiguration) -> Self {
        let shared = PtyShared {
            config: PtyConfigSnapshot {
                target_member_id: target_member.id.clone(),
                cols: config.cols,
                rows: config.rows,
                prompt_regex: None,
                prompt_inspect_lines: 3,
                prompt_idle_column: None,
                prime_timeout_ms: config.prime_timeout_ms,
                wedge_detection: config.wedge_detection,
            },
            last_byte_atomic: Arc::new(AtomicU64::new(0)),
            snapshot_tx: mpsc::channel(64).0,
        };
        Self {
            target_member,
            shared,
            write_tx: None,
            bytes_tx: None,
            ready: Arc::new(AtomicBool::new(false)),
            shutdown_flag: Arc::new(AtomicBool::new(false)),
            worker_handle: None,
            reader_handle: None,
            child: None,
        }
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
        // Per-coder initial_command arrives in §6 via PtyTargetConfiguration;
        // for the §4 skeleton we use a placeholder so the skeleton
        // runs end to end.
        let initial_command = if self.shared.config.target_member_id.is_empty() {
            "/bin/bash".to_string()
        } else {
            format!(
                "/bin/bash -lc 'exec -a {}'",
                self.shared.config.target_member_id
            )
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
        if let Some(cwd) = context.runtime_directory.to_str() {
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
            last_byte_atomic: self.shared.last_byte_atomic.clone(),
            snapshot_tx,
        };

        let target_session = self.target_member.id.clone();
        let target_session_for_reader = target_session.clone();
        let writer_for_worker = writer_arc.clone();
        let child_for_worker = child_arc.clone();
        let shutdown_flag_for_worker = self.shutdown_flag.clone();

        let bytes_tx_for_reader = bytes_tx.clone();
        let _writer_for_reader = writer_arc.clone();
        let last_byte_atomic_for_reader = self.shared.last_byte_atomic.clone();
        let reader_shutdown_flag = self.shutdown_flag.clone();

        let worker_handle = thread::Builder::new()
            .name(format!("pty-worker-{target_session}"))
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
                    target_session,
                    shutdown_flag_for_worker,
                );
            })
            .map_err(|e| TransportError {
                code: "pty_worker_spawn_failed".to_string(),
                reason: format!("worker thread spawn: {e}"),
                details: None,
            })?;

        let reader_handle = thread::Builder::new()
            .name(format!("pty-reader-{target_session_for_reader}"))
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
            // Channel closed; the consumer is gone. Same as mailw.
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
    _shared: PtyShared,
    writer: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
    child: Arc<std::sync::Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    target_session: String,
    shutdown_flag: Arc<AtomicBool>,
) {
    let _ = (writer, child);

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

    let writer_for_handler = _writer_or_clone();
    install_minimal_handlers(&mut terminal, writer_for_handler);

    while !shutdown_flag.load(Ordering::Acquire) {
        // Priority: snapshot > write > bytes.
        if let Ok(request) = snapshot_rx.try_recv() {
            handle_snapshot(&mut terminal, request);
            continue;
        }
        if let Ok(cmd) = write_rx.try_recv() {
            handle_delivery_command(cmd);
            continue;
        }
        if let Ok(bytes) = bytes_rx.try_recv() {
            terminal.vt_write(&bytes);
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

/// Helper to clone the writer Arc from the run_worker parameters.
/// Avoids an "unused variable" warning on the `_ = (writer, child)`
/// binding line while still letting us hand a writer to the effect
/// handler installer.
fn _writer_or_clone() -> Arc<std::sync::Mutex<Box<dyn Write + Send>>> {
    // This is unreachable in practice — `_ = (writer, child)` above
    // drops both. The function exists only so the closure below can
    // keep its `Arc::clone` call for §4.3 follow-up.
    Arc::new(std::sync::Mutex::new(Box::new(std::io::sink())))
}

fn install_minimal_handlers(
    terminal: &mut libghostty_vt::Terminal<'_, '_>,
    _writer: Arc<std::sync::Mutex<Box<dyn Write + Send>>>,
) {
    // The minimal handler for the §4 skeleton: on_pty_write does
    // nothing (responses can be sent later via a writer capture once
    // §4.3 wires the full handler set). The full on_size /
    // on_device_attributes / on_xtversion / on_title callbacks land
    // in §4.3 alongside the delivery-task rendering pipeline.
    let _ = terminal.on_pty_write(|_t, _data| {
        // No-op for v1 skeleton.
    });
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
        Ok(mut formatter) => match formatter.format_alloc(None) {
            Ok(bytes) => bytes.as_ref().to_vec(),
            Err(_) => Vec::new(),
        },
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

fn handle_delivery_command(cmd: DeliveryCommand) {
    // The full delivery task (render envelope, coalesce group, wait
    // for quiescence via the shared wedge/prime state machine) lands
    // in §4.4 as a follow-up. For now the command is consumed and the
    // outcome sender is dropped without resolution.
    match cmd {
        DeliveryCommand::Envelope { outcome_tx, .. } => drop(outcome_tx),
        DeliveryCommand::Raw { outcome_tx, .. } => drop(outcome_tx),
    }
}

fn drain_remaining(write_rx: &mut mpsc::Receiver<DeliveryCommand>, _target_session: &str) {
    while write_rx.try_recv().is_ok() {}
}

fn run_reader(
    mut reader: Box<dyn std::io::Read + Send>,
    bytes_tx: mpsc::Sender<Vec<u8>>,
    last_byte_atomic: Arc<AtomicU64>,
    shutdown_flag: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; 4096];
    while !shutdown_flag.load(Ordering::Acquire) {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                let now_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                last_byte_atomic.store(now_unix_ms, Ordering::Release);
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

#[allow(unused_imports)]
use crate::envelope::PromptBatchSettings as _ReexportBatchSettings;
