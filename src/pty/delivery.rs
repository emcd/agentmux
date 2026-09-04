//! Pty's half of the delivery-loop executor: the terminal it owns, what it
//! writes, and what writing proves.
//!
//! The loop itself is [`run_delivery_executor`](crate::transports::run_delivery_executor)
//! and is shared with every other transport. Pty differs from the other three in
//! one structural way, and it shapes this whole module: `libghostty_vt` types are
//! `!Send`, so the terminal lives on the executor's own thread. That makes this
//! thread the one that feeds the terminal its child's output and answers the
//! snapshot requests `look` and the prompt probe make — work that has to keep
//! happening while the executor is otherwise idle, which is what
//! [`DeliveryWriter::wait_for_work`] is overridden for.
//!
//! **Every member is its own packing unit**, because the transport writes each
//! one with its own `write_all` pair. Coalescing several into one unit would
//! smear a partial write's evidence across all of them: the bytes of an earlier
//! member could have landed while a later member's did not, and one unit can
//! report only one answer for both.

use std::{
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use crate::protocol::DeliveryDoorbell;
use crate::protocol::mailbox::{MailboxEntry, MailboxPayload};
use crate::transports::{
    DeliveryWriter, PeekDimensions, PlannedWrite, SubmissionEvidence, TransportHealth,
    UnreachableSince, WorkerReadinessState,
};

use super::state::{
    LOOK_LINES_DEFAULT, PtyShared, SnapshotRequest, SnapshotResponse, prompt_satisfied,
};

const RECEIPT_MARKER: &str = "--- agentmux terminal-outcome receipt ---";

/// How long the idle wait sleeps between servicing passes.
///
/// Short relative to the executor's poll interval, because what it paces is the
/// terminal's own responsiveness: a snapshot request answered a whole poll
/// interval late is a `look` that reads as hung.
const SERVICE_SLICE: Duration = Duration::from_millis(5);

/// What one iteration decided to write.
pub struct PtyWrite {
    /// The complete byte sequence for this member, buffered so the write is one
    /// primitive rather than a sequence a failure could land halfway through.
    bytes: Vec<u8>,
}

/// The Pty transport's contribution to the shared delivery loop.
///
/// Owns the terminal outright, which is why this type is constructed on the
/// executor thread and never leaves it.
pub struct PtyDeliveryWriter {
    terminal: libghostty_vt::Terminal<'static, 'static>,
    /// Bytes the reader thread captured from the child, fed to the terminal
    /// between iterations.
    bytes_rx: mpsc::Receiver<Vec<u8>>,
    /// Snapshot requests from `look` and the prompt probe, answered here because
    /// this thread holds the terminal they read.
    snapshot_rx: mpsc::Receiver<SnapshotRequest>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    shared: PtyShared,
    readiness: Arc<Mutex<WorkerReadinessState>>,
    mirror_state: Option<super::transport::PtyMirrorStateFn>,
    /// The transport's own latch, shared rather than duplicated. The dwell is
    /// measured from the instant this returns, so a second latch — or none —
    /// would restart `since` on every poll and the dwell could never elapse.
    unreachable_since: Arc<UnreachableSince>,
    /// Whether the child's departure has been published yet. The condition is
    /// level-triggered and this loop reads it every iteration; the readiness
    /// registry wants the edge.
    departure_published: bool,
    shutdown_flag: Arc<AtomicBool>,
}

impl PtyDeliveryWriter {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        terminal: libghostty_vt::Terminal<'static, 'static>,
        bytes_rx: mpsc::Receiver<Vec<u8>>,
        snapshot_rx: mpsc::Receiver<SnapshotRequest>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        shared: PtyShared,
        readiness: Arc<Mutex<WorkerReadinessState>>,
        mirror_state: Option<super::transport::PtyMirrorStateFn>,
        unreachable_since: Arc<UnreachableSince>,
        shutdown_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            terminal,
            bytes_rx,
            snapshot_rx,
            writer,
            shared,
            readiness,
            mirror_state,
            unreachable_since,
            departure_published: false,
            shutdown_flag,
        }
    }

    /// Feeds the terminal whatever the child wrote, answers whatever asked to
    /// read it, and publishes the child's departure the first time it is seen.
    ///
    /// Runs at every point the executor could otherwise be parked, and — unlike
    /// the readiness check — runs whatever the target's health says. The order is
    /// deliberate: bytes first, so a snapshot answered in the same pass reflects
    /// output that had already arrived rather than the state before it.
    ///
    /// The departure publish is here rather than in `is_ready` because
    /// `is_ready` is only consulted while the target is `Healthy`, and rather
    /// than in whatever ends the executor because a departed child deliberately
    /// does *not* end it — the executor stays to carry the dwell. This is the one
    /// pass that both runs unconditionally and can observe the transition.
    pub(crate) fn service_terminal(&mut self) {
        while let Ok(bytes) = self.bytes_rx.try_recv() {
            self.terminal.vt_write(&bytes);
        }
        while let Ok(request) = self.snapshot_rx.try_recv() {
            let response = render_snapshot(&mut self.terminal, request.inspect_lines);
            let _ = request.tx.send(response);
        }
        if !self.departure_published && self.shared.child_exited.load(Ordering::Acquire) {
            self.publish(WorkerReadinessState::Unavailable);
            self.departure_published = true;
        }
    }

    fn publish(&self, state: WorkerReadinessState) {
        if let Ok(mut readiness) = self.readiness.lock() {
            *readiness = state;
        }
        if let Some(mirror) = self.mirror_state.as_ref() {
            mirror(state);
        }
    }

    /// Evaluates this target's prompt-readiness template against the terminal.
    ///
    /// Read directly rather than through the snapshot channel, because this
    /// thread *is* the one that would answer such a request: routing a question
    /// to oneself and waiting for the reply is a deadlock, not an abstraction.
    /// Only the observation is local — the predicate is the shared
    /// [`prompt_satisfied`], so a `look`-side caller reading through the channel
    /// and this executor reading the terminal cannot come to different answers
    /// about the same snapshot.
    fn prompt_ready(&mut self) -> bool {
        let inspect = usize::from(self.shared.config.prompt_inspect_lines);
        let snapshot = render_snapshot(&mut self.terminal, Some(inspect));
        prompt_satisfied(&self.shared.config, &snapshot)
    }
}

impl DeliveryWriter for PtyDeliveryWriter {
    type Plan = PtyWrite;

    fn peek_dimensions(&self) -> PeekDimensions {
        PeekDimensions::for_session_type(crate::configuration::SessionType::Pty)
            .expect("pty declares peek dimensions")
    }

    fn health(&self) -> TransportHealth {
        // The child having exited is the one thing this side can positively
        // observe about reachability. Folded through the transport's own latch
        // rather than reported fresh: the dwell is measured from the `since` this
        // returns, so a stamp taken here on every poll would keep resetting the
        // clock and the dwell could never elapse — the entries a departed child
        // can never receive would wait forever for an outcome they are owed.
        self.unreachable_since
            .fold(!self.shared.child_exited.load(Ordering::Acquire))
    }

    fn is_ready(&mut self) -> bool {
        // Serviced first, so the readiness reading below is taken against
        // everything the child has written rather than against the terminal as it
        // stood before this iteration.
        self.service_terminal();
        if self.shared.child_exited.load(Ordering::Acquire) {
            return false;
        }
        self.prompt_ready()
    }

    fn plan(&mut self, entries: &[MailboxEntry]) -> Option<PlannedWrite<Self::Plan>> {
        let head = entries.first()?;
        let bytes = match &head.payload {
            MailboxPayload::Mail(envelope) => {
                let text = envelope.message.render_pane_envelope(&envelope.message_id);
                let mut buffer = Vec::with_capacity(text.len() + RECEIPT_MARKER.len() + 2);
                if envelope.is_receipt {
                    buffer.extend_from_slice(RECEIPT_MARKER.as_bytes());
                    buffer.push(b'\n');
                }
                buffer.extend_from_slice(text.as_bytes());
                buffer.push(b'\n');
                buffer
            }
            MailboxPayload::Raw {
                content,
                append_enter,
            } => {
                let mut buffer = Vec::with_capacity(content.len() + 1);
                buffer.extend_from_slice(content.as_bytes());
                if *append_enter {
                    buffer.push(b'\n');
                }
                buffer
            }
        };
        // One entry, always. The declaration the relay records therefore covers
        // exactly what one `write_all` carries, which is what lets a member's
        // evidence be its own write's result rather than a groupmate's.
        Some(PlannedWrite {
            entry_count: 1,
            rendered: PtyWrite { bytes },
        })
    }

    fn write(&mut self, planned: PlannedWrite<Self::Plan>) -> Vec<SubmissionEvidence> {
        self.publish(WorkerReadinessState::Busy);
        let result = (|| -> std::io::Result<()> {
            let mut guard = self
                .writer
                .lock()
                .map_err(|_| std::io::Error::other("pty writer mutex poisoned"))?;
            guard.write_all(&planned.rendered.bytes)
        })();
        self.publish(WorkerReadinessState::Available);
        vec![match result {
            Ok(()) => SubmissionEvidence::Submitted,
            // `write_all` reports how many bytes it wrote only on success, so a
            // failure cannot exclude a partial write reaching the master.
            // `NotSubmitted` would be a positive claim this transport cannot
            // support once the write has begun.
            Err(_) => SubmissionEvidence::SubmissionUnknown,
        }]
    }

    fn stop_requested(&mut self) -> bool {
        // A departed child is deliberately NOT a stop. It is an unreachability,
        // and unreachability is what the dwell exists to carry: the executor has
        // to stay so `health` keeps reporting it and the loop can resolve the
        // entries once the dwell elapses. Ending here instead would leave every
        // queued entry — and every entry admitted afterwards — with no executor
        // to peek it and no outcome ever reported for it.
        //
        // The generation still ends promptly: `shutdown` and the fence both latch
        // this flag, and the fence's forced step kills the child, which is what
        // wakes an executor parked in a write into the master.
        self.shutdown_flag.load(Ordering::Acquire)
    }

    fn wait_for_work(&mut self, doorbell: &DeliveryDoorbell, timeout: Duration) {
        // The terminal keeps being fed and keeps answering while this executor
        // waits, which no other transport has to arrange because no other
        // transport's idle thread owns anything. The wait is sliced rather than
        // taken whole for exactly that reason: a snapshot request answered one
        // poll interval late is a `look` that reads as hung.
        let deadline = Instant::now() + timeout;
        loop {
            self.service_terminal();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return;
            }
            if doorbell.wait_for(remaining.min(SERVICE_SLICE)) {
                return;
            }
        }
    }
}

/// Renders the terminal's plain-text tail and cursor position.
fn render_snapshot(
    terminal: &mut libghostty_vt::Terminal<'_, '_>,
    inspect_lines: Option<usize>,
) -> SnapshotResponse {
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
    let lines_to_take = inspect_lines.unwrap_or(LOOK_LINES_DEFAULT);
    let mut collected: Vec<String> = tail
        .lines()
        .rev()
        .take(lines_to_take)
        .map(str::to_string)
        .collect();
    collected.reverse();
    SnapshotResponse {
        tail: collected.join("\n"),
        cursor_x: terminal.cursor_x().unwrap_or(0),
        cursor_y: terminal.cursor_y().unwrap_or(0),
        cursor_visible: terminal.is_cursor_visible().unwrap_or(false),
    }
}
