//! NDJSON framing, outbox bounds, and the control lane.
//!
//! Reads newline-delimited UTF-8 JSON-RPC 2.0 frames, bounding the
//! accumulated inbound bytes before the terminating newline (no length
//! prefix on the wire): the 64KiB bootstrap ceiling before negotiation,
//! the negotiated `max_frame_bytes` as the operative bound after. Owns
//! the transmit queue caps with the perspective rule, the bounded drain
//! wait, and the control lane exempt from data caps within its own bound.

//! The gated reader lives here too: it acquires one frame at a time
//! under the operative bound, parks on RX queue backpressure instead
//! of growing memory, and releases on shutdown.

use std::{
    error::Error,
    fmt::{Display, Formatter},
    io::{self, BufRead, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

/// Pre-negotiation inbound frame ceiling: the bootstrap bound that
/// applies to the hello exchange before negotiated limits exist.
pub const BOOTSTRAP_MAX_FRAME_BYTES: usize = 64 * 1024;

/// Framing failure for NDJSON read, parse, and write paths.
#[derive(Debug)]
pub enum FrameError {
    Oversize { bound: usize },
    Unparseable { reason: String },
    MessageTooLarge { bound: usize },
    Io { context: String, source: io::Error },
}

impl FrameError {
    fn oversize(bound: usize) -> Self {
        Self::Oversize { bound }
    }

    fn unparseable(reason: impl Into<String>) -> Self {
        Self::Unparseable {
            reason: reason.into(),
        }
    }

    fn message_too_large(bound: usize) -> Self {
        Self::MessageTooLarge { bound }
    }

    /// Wraps an I/O failure with the operation it interrupted.
    pub fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl Display for FrameError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Oversize { bound } => {
                write!(
                    formatter,
                    "frame exceeds the inbound bound of {bound} bytes"
                )
            },
            Self::Unparseable { reason } => {
                write!(formatter, "frame is not parseable JSON: {reason}")
            },
            Self::MessageTooLarge { bound } => {
                write!(
                    formatter,
                    "outbound frame exceeds the bound of {bound} bytes"
                )
            },
            Self::Io { context, source } => {
                write!(formatter, "{context}: {source}")
            },
        }
    }
}

impl Error for FrameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Reads one NDJSON frame, returning its bytes without the terminator.
///
/// Returns `Ok(None)` on clean EOF (no bytes pending). Bytes accumulate
/// only up to `bound + 1`; a frame that outgrows the bound fails as
/// [`FrameError::Oversize`] without ever holding a body-sized buffer. A
/// nonempty stream that ends before any newline fails as unparseable.
///
/// # Errors
///
/// Returns [`FrameError::Oversize`] past the bound, [`FrameError::Io`]
/// on read failure, [`FrameError::Unparseable`] on a truncated frame.
pub fn read_frame(
    reader: &mut (impl BufRead + ?Sized),
    bound: usize,
) -> Result<Option<Vec<u8>>, FrameError> {
    let mut frame: Vec<u8> = Vec::new();
    // Byte-wise over the buffered reader: one syscall per buffer fill,
    // and bytes past the newline stay buffered for the next frame.
    // Allocation never exceeds `bound + 1` no matter how large the
    // peer's frame grows.
    let mut byte = [0_u8; 1];
    loop {
        match reader.read_exact(&mut byte) {
            Ok(()) => {},
            Err(source) if source.kind() == io::ErrorKind::UnexpectedEof => {
                break;
            },
            Err(source) => {
                return Err(FrameError::io("read frame", source));
            },
        }
        if byte[0] == b'\n' {
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            // A bound+1 body is refused even though it fits the
            // accumulation cap: the cap bounds allocation, the bound
            // bounds the frame. CRLF endings are stripped first, so
            // the carriage return never counts against the frame.
            if frame.len() > bound {
                return Err(FrameError::oversize(bound));
            }
            return Ok(Some(frame));
        }
        if frame.len() >= bound.saturating_add(1) {
            return Err(FrameError::oversize(bound));
        }
        frame.push(byte[0]);
    }
    if frame.is_empty() {
        return Ok(None);
    }
    Err(FrameError::unparseable(
        "stream ended before the frame newline",
    ))
}

/// Writes one frame plus its newline and flushes, so the peer can read
/// promptly. Payloads over `bound` fail before any byte is written.
///
/// # Errors
///
/// Returns [`FrameError::MessageTooLarge`] past the bound and
/// [`FrameError::Io`] on write or flush failure.
pub fn write_frame(
    writer: &mut (impl Write + ?Sized),
    payload: &[u8],
    bound: usize,
) -> Result<(), FrameError> {
    if payload.len() > bound {
        return Err(FrameError::message_too_large(bound));
    }
    writer
        .write_all(payload)
        .map_err(|source| FrameError::io("write frame", source))?;
    writer
        .write_all(b"\n")
        .map_err(|source| FrameError::io("write frame newline", source))?;
    writer
        .flush()
        .map_err(|source| FrameError::io("flush frame", source))
}

/// Parses frame bytes as JSON.
///
/// # Errors
///
/// Returns [`FrameError::Unparseable`] for empty input or invalid JSON.
pub fn parse_frame(bytes: &[u8]) -> Result<Value, FrameError> {
    if bytes.is_empty() {
        return Err(FrameError::unparseable("empty frame carries no JSON"));
    }
    serde_json::from_slice(bytes)
        .map_err(|source| FrameError::unparseable(source.to_string()))
}

/// Builds the contract `-32000` JSON-RPC error object for a failed call.
///
/// `id` echoes the failed request's id when the frame supplied one;
/// `correlation_id` echoes the failed request's correlation when the
/// request carried one — except `correlation_invalid`, which is exempt
/// from the required echo and is admitted by pending-table match
/// instead.
#[must_use]
pub fn contract_error(
    id: Option<&Value>,
    code: &str,
    reason: &str,
    details: Option<Value>,
    correlation_id: Option<&str>,
) -> Value {
    let mut data = serde_json::Map::new();
    data.insert("code".to_string(), Value::String(code.to_string()));
    data.insert("reason".to_string(), Value::String(reason.to_string()));
    if let Some(details) = details {
        data.insert("details".to_string(), details);
    }
    if let Some(correlation_id) = correlation_id {
        data.insert(
            "correlation_id".to_string(),
            Value::String(correlation_id.to_string()),
        );
    }
    json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(Value::Null),
        "error": {
            "code": -32000,
            "message": reason,
            "data": Value::Object(data),
        },
    })
}

#[derive(Debug)]
pub(crate) enum ReadEvent {
    Frame(Vec<u8>),
    /// An oversize frame was drained past its newline and refused:
    /// nothing retained, nothing answered. The run loop records the
    /// typed framing failure and continues the session.
    Discarded {
        bound: usize,
    },
    Eof,
    Unusable,
}

/// Shared run-loop state for the reader thread: the operative read
/// bound, the install flag and gate, the shutdown flag, and the RX
/// queue byte accounting. Grouped so the reader entry keeps a narrow
/// signature.
pub(crate) struct ReaderShared {
    pub bound: Arc<AtomicUsize>,
    pub installed: Arc<AtomicBool>,
    pub done: Arc<AtomicBool>,
    pub rx_queued: Arc<AtomicUsize>,
    pub rx_budget: Arc<AtomicUsize>,
}

/// Gated reader: exactly one frame is acquired before the binding
/// exists, then the run loop releases the next acquisition per frame
/// it processes, so the negotiated phase transition is ordered before
/// post-hello frame acquisition even when the hello result and the
/// first operation arrive coalesced. After installation the gate
/// opens permanently and acquisition runs under the installed bound.
///
/// The read channel is a depth-one handoff, not a bound: capacity
/// one only sequences delivery. The aggregate byte bound comes
/// solely from the budget gate below — the reader parks instead of
/// reading once queued-but-undispatched bytes plus the next frame
/// would exceed the RX queue byte cap. Together the per-frame
/// bound (lesser of frame cap and RX queue cap) and the budget gate
/// bound aggregate buffered bytes by the RX queue cap exactly —
/// backpressure, never unbounded queueing. The reader releases on
/// shutdown and never outlives the run.
/// Outcome of draining an oversize frame past its newline.
enum DrainOutcome {
    /// Newline reached; the post-newline remainder (always empty on
    /// the pre-bind single-byte stride) returns stashed for the next
    /// frame.
    Discarded(Vec<u8>),
    /// Transport closed before any newline.
    Eof,
    /// Transport failed mid-drain.
    Unusable,
    /// Shutdown requested mid-drain.
    Shutdown,
}

/// Discards an oversize frame through its terminating newline without
/// retaining it: the bound that refused the frame bounds nothing
/// here. Pre-bind drains one byte at a time so no bulk-read stash
/// can accumulate under the bootstrap budget and survive into the
/// negotiated bound; post-bind drains in chunks and returns the
/// post-newline remainder. EOF before the newline ends the drain as
/// EOF (the session ends by EOF, clean); a read failure is a dead
/// transport.
fn drain_oversize(
    reader: &mut (impl Read + ?Sized),
    chunk: &mut [u8; 4096],
    installed: bool,
    retain_cap: usize,
    done: &AtomicBool,
) -> DrainOutcome {
    let stride = if installed {
        chunk.len().min(retain_cap.saturating_add(1)).max(1)
    } else {
        1
    };
    loop {
        if done.load(Ordering::Acquire) {
            return DrainOutcome::Shutdown;
        }
        let count = match reader.read(&mut chunk[..stride]) {
            Ok(0) => return DrainOutcome::Eof,
            Ok(count) => count,
            Err(_) => return DrainOutcome::Unusable,
        };
        if let Some(index) =
            chunk[..count].iter().position(|byte| *byte == b'\n')
        {
            return DrainOutcome::Discarded(chunk[index + 1..count].to_vec());
        }
    }
}

/// Depth-one rendezvous handoff with shutdown release: parks
/// while the run loop has not taken the event. Returns false when the
/// run loop is gone, in which case the caller returns.
fn rendezvous(
    tx: &SyncSender<ReadEvent>,
    done: &AtomicBool,
    event: ReadEvent,
) -> bool {
    let mut pending = Some(event);
    while let Some(current) = pending.take() {
        match tx.try_send(current) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Full(event)) => {
                if done.load(Ordering::Acquire) {
                    return false;
                }
                pending = Some(event);
                thread::sleep(Duration::from_millis(1));
            },
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
        }
    }
    true
}

/// Gated NDJSON reader; see run-loop documentation for the
/// ordering, handoff, and budget rules.
pub(crate) fn read_loop(
    reader: impl Read + Send,
    tx: SyncSender<ReadEvent>,
    gate: Receiver<()>,
    shared: ReaderShared,
) {
    let ReaderShared {
        bound,
        installed,
        done,
        rx_queued,
        rx_budget,
    } = &shared;
    let mut reader = reader;
    // Chunk reads bound transport acquisition exactly: only the frame
    // vector retains bytes (plus kernel pipe buffers outside our
    // accounting), so no BufRead layer may sit between the transport
    // and this loop or bulk buffering would defeat the byte budget.
    // A single one-byte lookahead past an exactly-consumed budget
    // decides completion versus refusal and is dropped immediately
    // on refusal — it is never admitted to the queue.
    let mut chunk = [0_u8; 4096];
    // Leftover bytes read past a frame's newline belong to the next
    // frame: they are stashed, never discarded, and folded into the
    // next frame's accumulation so the budget gate keeps accounting
    // them.
    let mut stash: Vec<u8> = Vec::new();
    'read: loop {
        let hard = bound.load(Ordering::Relaxed);
        // Budget-gated acquisition: accumulate the next frame without
        // ever retaining more than the remaining RX queue budget.
        // Retained partial bytes plus queued-but-undispatched bytes
        // never exceed the RX queue cap by construction: the reader
        // parks instead of growing memory when the run loop lags, and
        // the hard per-frame bound still refuses oversize frames
        // outright rather than parking them behind a budget that can
        // never cover them. Shutdown releases a parked reader.
        let mut frame: Vec<u8> = std::mem::take(&mut stash);
        // A stashed remainder may already hold complete frames from
        // a bulk read: resolve them before touching the transport,
        // subject to the same hard bound and queue budget as a fresh
        // read — stashing is retention, not admission.
        let event = if let Some(index) =
            frame.iter().position(|byte| *byte == b'\n')
        {
            let mut completed = frame[..index].to_vec();
            if completed.last() == Some(&b'\r') {
                completed.pop();
            }
            if completed.len() > hard {
                // Refuse the frame, keep the session: the post-newline
                // remainder still belongs to the next frame.
                stash.extend_from_slice(&frame[index + 1..]);
                ReadEvent::Discarded { bound: hard }
            } else {
                let queued_now = rx_queued.load(Ordering::Acquire);
                let mut admitted = queued_now + completed.len()
                    <= rx_budget.load(Ordering::Relaxed);
                while !admitted {
                    if queued_now == 0 || done.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                    admitted = rx_queued.load(Ordering::Acquire)
                        + completed.len()
                        <= rx_budget.load(Ordering::Relaxed);
                }
                if !admitted && queued_now > 0 {
                    // Still over budget with nothing left to free it:
                    // refuse rather than parking behind a wait that
                    // cannot end. (queued_now == 0 with an over-budget
                    // frame is impossible here: completed.len() <=
                    // hard <= budget when the main loop holds nothing.)
                    stash.clear();
                    ReadEvent::Unusable
                } else {
                    stash.extend_from_slice(&frame[index + 1..]);
                    ReadEvent::Frame(completed)
                }
            }
        } else {
            loop {
                // Fast refusal first: past the hard frame cap plus one
                // byte for a possible CR the frame is oversize no
                // matter what the budget later frees. The extra byte
                // is the CRLF terminator's carriage return, decided
                // only when the newline arrives: the exact post-strip
                // bound is enforced below, so an exact-bound CRLF
                // frame is admitted like its LF twin. At exactly
                // cap plus one with no trailing CR the verdict is
                // already certain too (only a terminator can follow,
                // and the body is over bound with or without a
                // stripped CR), so that state drains as well instead
                // of falling through to the budget refusal below.
                let over = frame.len() > hard.saturating_add(1)
                    || (frame.len() == hard.saturating_add(1)
                        && frame.last() != Some(&b'\r'));
                if over {
                    // The bound is certainly exceeded: publish the
                    // framing failure FIRST, independently of whether
                    // delimiter resynchronization subsequently
                    // completes. A peer that holds the pipe open with
                    // no further bytes must not take the detected
                    // oversize down with its silence: handshake and
                    // liveness timeouts close such sessions on their
                    // own deadlines with the evidence already
                    // recorded. Discarded bytes never prove a live
                    // peer, so liveness is deliberately untouched.
                    if !rendezvous(
                        &tx,
                        done,
                        ReadEvent::Discarded { bound: hard },
                    ) {
                        return;
                    }
                    if !installed.load(Ordering::Acquire) {
                        if done.load(Ordering::Acquire) {
                            return;
                        }
                        if gate.recv().is_err() {
                            return;
                        }
                    }
                    let installed_now = installed.load(Ordering::Acquire);
                    // Snapshot cap: the run loop only releases budget
                    // while the reader drains, so a cap taken now still
                    // bounds the retained tail later.
                    let retain_cap = if installed_now {
                        rx_budget
                            .load(Ordering::Relaxed)
                            .saturating_sub(rx_queued.load(Ordering::Acquire))
                    } else {
                        0
                    };
                    match drain_oversize(
                        &mut reader,
                        &mut chunk,
                        installed_now,
                        retain_cap,
                        done,
                    ) {
                        DrainOutcome::Discarded(remainder) => {
                            // The refused prefix stays refused: drop
                            // it and keep only the post-newline tail.
                            // Resume from the outer loop so the
                            // stashed tail resolves before any further
                            // transport read.
                            frame.clear();
                            stash.extend_from_slice(&remainder);
                            continue 'read;
                        },
                        // The terminal cause is already decided — and
                        // the oversize already published — so close
                        // with it directly instead of rediscovering it
                        // with another source read, which could swallow
                        // a one-shot error and answer later frames.
                        DrainOutcome::Eof => {
                            break ReadEvent::Eof;
                        },
                        DrainOutcome::Unusable => {
                            break ReadEvent::Unusable;
                        },
                        DrainOutcome::Shutdown => return,
                    }
                }
                let queued_now = rx_queued.load(Ordering::Acquire);
                let remaining = rx_budget
                    .load(Ordering::Relaxed)
                    .saturating_sub(queued_now);
                // At the budget edge with bytes still queued, the main
                // loop may free budget: park instead of retaining more.
                // With nothing outstanding, no dispatch can free budget,
                // so an incomplete frame already past the remaining
                // budget can never fit: refuse it rather than parking
                // behind a wait that cannot end.
                if frame.len() >= remaining && queued_now > 0 {
                    if done.load(Ordering::Acquire) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                if frame.len() > remaining {
                    // A single trailing carriage return may still be a
                    // CRLF terminator whose line feed arrives next, so
                    // one lookahead byte is allowed before deciding.
                    // Anything else past the edge can never fit:
                    // refuse it rather than parking behind a wait that
                    // cannot end. (With queued bytes the park above
                    // already fired, so this branch always sees an
                    // empty queue.)
                    let cr_lookahead = frame.len()
                        == remaining.saturating_add(1)
                        && frame.last() == Some(&b'\r');
                    if !cr_lookahead {
                        break ReadEvent::Unusable;
                    }
                }
                // Pre-bind, only the hello reply is expected: read
                // byte by byte and stop exactly at its newline, never
                // retaining beyond it. This keeps the phase barrier
                // exact — no bulk-read stash can accumulate under the
                // bootstrap budget and survive into the negotiated
                // bound. Post-bind, chunked reads amortize syscalls;
                // the budget gate keeps every chunk within the
                // remaining RX queue bytes.
                let stride = if installed.load(Ordering::Acquire) {
                    4096
                } else {
                    1
                };
                let take = stride
                    .min((hard + 1).saturating_sub(frame.len()))
                    .min(remaining.saturating_sub(frame.len()))
                    .max(1);
                let count = match reader.read(&mut chunk[..take]) {
                    Ok(0) => {
                        break if frame.is_empty() {
                            ReadEvent::Eof
                        } else {
                            ReadEvent::Unusable
                        };
                    },
                    Ok(count) => count,
                    Err(_) => break ReadEvent::Unusable,
                };
                match chunk[..count].iter().position(|byte| *byte == b'\n') {
                    Some(index) => {
                        frame.extend_from_slice(&chunk[..index]);
                        if frame.last() == Some(&b'\r') {
                            frame.pop();
                        }
                        stash.extend_from_slice(&chunk[index + 1..count]);
                        if frame.len() > hard {
                            // The head is refused; the stashed tail
                            // still belongs to the next frame.
                            break ReadEvent::Discarded { bound: hard };
                        }
                        break ReadEvent::Frame(frame);
                    },
                    None => frame.extend_from_slice(&chunk[..count]),
                }
            }
        };
        let terminal = matches!(event, ReadEvent::Eof | ReadEvent::Unusable);
        if let ReadEvent::Frame(bytes) = &event {
            rx_queued.fetch_add(bytes.len(), Ordering::AcqRel);
        }
        // Rendezvous handoff with shutdown release: park while the run
        // loop has not yet taken the previous frame.
        if !rendezvous(&tx, done, event) {
            return;
        }
        if terminal {
            return;
        }
        if !installed.load(Ordering::Acquire) {
            // Pre-bind: wait for the run loop to process this frame.
            // The run loop answers every pre-install frame it receives;
            // a dropped gate or a set done flag means the run loop is
            // gone.
            if done.load(Ordering::Acquire) {
                return;
            }
            if gate.recv().is_err() {
                return;
            }
        }
    }
}
/// Reader backpressure is structural: no public interface can stall
/// the run loop deterministically (it always drains), so the parked
/// acquisition path is unreachable from `run_stdio` tests. This
/// single test drives `read_loop` directly — crate-private by
/// design, never public API — through park, budget release, and
/// shutdown. One test function by the inline-test rule.
#[cfg(test)]
mod reader_budget_tests {
    use std::io::Read;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::read_loop;
    use super::{BOOTSTRAP_MAX_FRAME_BYTES, ReadEvent, ReaderShared};

    /// Counts bytes the reader transport actually yields: acquisition
    /// beyond this count would mean retained-but-undispatched bytes
    /// the queue bound does not cover.
    struct CountingReader<R> {
        inner: R,
        count: Arc<AtomicUsize>,
    }

    impl<R: Read> Read for CountingReader<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(buf)?;
            self.count.fetch_add(n, Ordering::Relaxed);
            Ok(n)
        }
    }

    /// Pre-bind acquisition stops exactly at the first newline:
    /// with 250 of 300 RX budget bytes queued, a 50-byte hello plus
    /// ten coalesced 100-byte future frames delivers only the hello
    /// (51 wire bytes) and retains nothing beyond it — the phase
    /// barrier cannot be established by a later parser refusal
    /// because no post-hello byte is ever acquired pre-bind. The
    /// parked-budget, release, coalesced-delivery, and shutdown
    /// phases below then exercise the steady-state gate.
    #[test]
    fn rx_budget_parks_acquisition_until_release() {
        // Phase 0: hello (50B + newline) coalesced with ten 100B
        // future frames behind it, read pre-bind.
        let mut prebind = vec![b'H'; 50];
        prebind.push(b'\n');
        for _ in 0..10 {
            prebind.extend_from_slice(&[b'F'; 100]);
            prebind.push(b'\n');
        }
        let pre_yielded = Arc::new(AtomicUsize::new(0));
        let pre_reader = CountingReader {
            inner: std::io::Cursor::new(prebind),
            count: pre_yielded.clone(),
        };
        let (pre_tx, pre_rx) = mpsc::sync_channel(1);
        let (pre_gate_tx, pre_gate_rx) = mpsc::channel::<()>();
        let pre_done = Arc::new(AtomicBool::new(false));
        // Fresh binding: nothing outstanding, so the full budget is
        // available and the only bound is the per-frame cap.
        let pre_queued = Arc::new(AtomicUsize::new(0));
        let pre_budget = Arc::new(AtomicUsize::new(300));
        let pre_handle = {
            let done = pre_done.clone();
            move || {
                read_loop(
                    pre_reader,
                    pre_tx,
                    pre_gate_rx,
                    ReaderShared {
                        bound: Arc::new(AtomicUsize::new(
                            BOOTSTRAP_MAX_FRAME_BYTES,
                        )),
                        installed: Arc::new(AtomicBool::new(false)),
                        done,
                        rx_queued: pre_queued,
                        rx_budget: pre_budget,
                    },
                );
            }
        };
        let pre_thread = std::thread::spawn(pre_handle);
        match pre_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(ReadEvent::Frame(bytes)) => {
                assert_eq!(bytes.len(), 50);
            },
            other => panic!("hello must arrive pre-bind: {other:?}"),
        }
        // Exactly the hello frame crossed the transport: single-byte
        // pre-bind reads retain nothing beyond its newline.
        assert_eq!(
            pre_yielded.load(Ordering::Acquire),
            51,
            "pre-bind acquisition stops at the first newline",
        );
        // Release the gate as a handshake would, then shut down: the
        // reader must not have retained the coalesced future.
        pre_gate_tx.send(()).expect("gate releases");
        pre_done.store(true, Ordering::Release);
        pre_thread.join().expect("reader releases on shutdown");
        // Phase 1: steady-state budget parking with coalesced frames.
        // Frame 1: 50 content bytes plus newline; frame 2: 100
        // content bytes plus newline, coalesced behind it.
        let mut wire = vec![b'{'; 50];
        wire.push(b'\n');
        wire.extend_from_slice(&[b'}'; 100]);
        wire.push(b'\n');
        assert_eq!(wire.len(), 152);
        let yielded = Arc::new(AtomicUsize::new(0));
        let reader = CountingReader {
            inner: std::io::Cursor::new(wire),
            count: yielded.clone(),
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let (_gate_tx, gate_rx) = mpsc::channel::<()>();
        let done = Arc::new(AtomicBool::new(false));
        let queued = Arc::new(AtomicUsize::new(250));
        let budget = Arc::new(AtomicUsize::new(300));
        let handle = std::thread::spawn({
            let done = done.clone();
            let queued = queued.clone();
            let budget = budget.clone();
            move || {
                read_loop(
                    reader,
                    tx,
                    gate_rx,
                    ReaderShared {
                        bound: Arc::new(AtomicUsize::new(
                            BOOTSTRAP_MAX_FRAME_BYTES,
                        )),
                        installed: Arc::new(AtomicBool::new(true)),
                        done,
                        rx_queued: queued,
                        rx_budget: budget,
                    },
                );
            }
        });
        // Parked: 250 + 50 > 300, so nothing arrives while the
        // main loop holds its budget.
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "over-budget frame must not be delivered",
        );
        // Retained acquisition never exceeds the remaining budget:
        // only the first 50 bytes may have left the transport.
        assert_eq!(
            yielded.load(Ordering::Acquire),
            50,
            "acquisition must stop at the budget edge",
        );
        // Release: the run loop dispatched 250 bytes.
        queued.store(0, Ordering::Release);
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(ReadEvent::Frame(bytes)) => {
                assert_eq!(bytes.len(), 50);
            },
            other => panic!("frame must arrive after release: {other:?}"),
        }
        // The coalesced second frame was stashed, budget-rechecked
        // (queued + 100 <= 300), and delivered in order with no
        // further transport reads beyond the two frames. No
        // intermediate queued assertion: the reader may deliver the
        // second frame before the test observes the first.
        match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(ReadEvent::Frame(bytes)) => {
                assert_eq!(bytes.len(), 100);
            },
            other => {
                panic!("coalesced frame must follow in order: {other:?}")
            },
        }
        assert_eq!(queued.load(Ordering::Acquire), 150);
        assert_eq!(
            yielded.load(Ordering::Acquire),
            152,
            "both frames acquired exactly once",
        );
        done.store(true, Ordering::Release);
        handle.join().expect("reader thread releases on shutdown");
        // Phase 2: an oversize head drains past its newline retaining
        // at most the remaining RX budget. Bound and budget are both
        // 300: a 301-byte oversize head plus ten coalesced 100-byte
        // future frames.
        let mut oversize = vec![b'X'; 301];
        oversize.push(b'\n');
        for _ in 0..10 {
            oversize.extend_from_slice(&[b'F'; 100]);
            oversize.push(b'\n');
        }
        let drain_yielded = Arc::new(AtomicUsize::new(0));
        let drain_reader = CountingReader {
            inner: std::io::Cursor::new(oversize),
            count: drain_yielded.clone(),
        };
        let (drain_tx, drain_rx) = mpsc::sync_channel(1);
        let (_drain_gate_tx, drain_gate_rx) = mpsc::channel::<()>();
        let drain_done = Arc::new(AtomicBool::new(false));
        let drain_queued = Arc::new(AtomicUsize::new(0));
        let drain_budget = Arc::new(AtomicUsize::new(300));
        let drain_handle = std::thread::spawn({
            let drain_done = drain_done.clone();
            let drain_queued = drain_queued.clone();
            let drain_budget = drain_budget.clone();
            move || {
                read_loop(
                    drain_reader,
                    drain_tx,
                    drain_gate_rx,
                    ReaderShared {
                        bound: Arc::new(AtomicUsize::new(300)),
                        installed: Arc::new(AtomicBool::new(true)),
                        done: drain_done,
                        rx_queued: drain_queued,
                        rx_budget: drain_budget,
                    },
                );
            }
        });
        match drain_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(ReadEvent::Discarded { bound }) => {
                assert_eq!(bound, 300);
            },
            other => panic!("oversize head must discard: {other:?}"),
        }
        // Acquisition at the discard is bounded: the 301-byte refused
        // prefix plus at most one budget-capped drain read. A drain
        // that prefetches the coalesced future beyond the 300-byte
        // budget would already have taken far more from the
        // transport here.
        assert!(
            drain_yielded.load(Ordering::Acquire) <= 301 + 301,
            "drain must not prefetch past the budget: {}",
            drain_yielded.load(Ordering::Acquire)
        );
        // All ten future frames still arrive whole and in order.
        for _ in 0..10 {
            match drain_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(ReadEvent::Frame(bytes)) => {
                    assert_eq!(bytes.len(), 100);
                    assert!(bytes.iter().all(|byte| *byte == b'F'));
                    // Mirror the run loop: dispatched bytes release
                    // their queue accounting.
                    drain_queued.fetch_sub(
                        bytes.len().min(drain_queued.load(Ordering::Acquire)),
                        Ordering::AcqRel,
                    );
                },
                other => {
                    panic!("future frame must follow in order: {other:?}")
                },
            }
        }
        // The transport then ends: every byte acquired exactly once,
        // nothing lost or duplicated across the discard.
        match drain_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(ReadEvent::Eof) => {},
            other => panic!("EOF must close the drained stream: {other:?}"),
        }
        assert_eq!(
            drain_yielded.load(Ordering::Acquire),
            301 + 1 + 10 * 101,
            "oversize prefix, newline, and future frames exactly once",
        );
        drain_done.store(true, Ordering::Release);
        drain_handle
            .join()
            .expect("reader thread releases on shutdown");
    }
}
