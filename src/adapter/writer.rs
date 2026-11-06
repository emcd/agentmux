//! Outbound frame writer for the reference adapter: the
//! threaded transport sink behind [`FrameSink`]. Owns the per-lane
//! queues, the control-first scheduling discipline with its
//! take-then-recheck and held-data rules, and the completion/death
//! reports consumed by run-loop queue accounting.

use std::io::Write;

/// Outbound lane for one enqueued frame: data frames count against the
/// peer's receive cap while control frames are accounted against
/// their own negotiated control bound, exempt from data caps. Lane
/// drives queue accounting and admission isolation; the writer serves
/// queued control ahead of further queued data at every intact frame
/// boundary, finishing any in-progress frame first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    Data,
    Control,
}

/// Completion or failure report from the frame-writer thread to the
/// run loop. The writer owns the transport; the run loop owns queue
/// accounting and releases it on each [`WriterEvent::Written`]. A
/// blocked or broken transport surfaces as [`WriterEvent::Dead`], and
/// the run loop terminalizes locally rather than answering over it.
pub(crate) enum WriterEvent {
    Written { lane: Lane, len: usize },
    Dead { context: String },
    Exited,
}

/// Outbound queues to the frame-writer thread: separate bounded
/// channels per lane, control-first at intact frame boundaries. The
/// separation is admission-side: a flooded data lane can never block
/// or refuse a control enqueue (fence, cancel, terminate, ack
/// admission, ping replies, lifecycle events). At every frame
/// boundary the writer serves a queued control frame ahead of
/// further queued data, never mid-frame: a data frame already being
/// written always finishes first, and queued control still waits for
/// it. Caps stay per-lane: control stays exempt from data caps
/// within its own negotiated control bound. Enqueue blocks only up
/// to the negotiated drain wait, then fails rather than queueing
/// unboundedly — a non-draining peer is treated as a dead transport.
pub(crate) struct FrameSink {
    pub data_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    pub ctrl_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    notify_tx: std::sync::mpsc::SyncSender<()>,
    /// Admitted-but-unflushed control frames: enqueued by the run
    /// loop into its deferred queue, not yet visible in the control
    /// channel the writer polls. The writer treats a nonzero count
    /// as queued control and holds not-yet-started data until the
    /// run loop flushes it into the channel, so deferred control is
    /// never bypassed by data the writer happens to take first.
    /// Updated only by the run loop (fetch_add before each deferred
    /// push, fetch_sub after each flush move or drop), so the count
    /// never understates the backlog; the writer only reads it.
    pub(crate) deferred_ctrl: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl FrameSink {
    /// Wakes a parked writer. Coalescing and non-blocking by
    /// construction: the wakeup channel holds at most one pending
    /// wakeup, and further notifies while one is queued are dropped
    /// rather than retained — a wakeup only means "recheck the
    /// lanes", so one pending wakeup carries every notification
    /// since. Returns whether this call queued the wakeup (false
    /// means one was already pending). Never blocks and never
    /// grows, no matter how many frames enqueue while the writer is
    /// busy; a dropped notify can never strand work because the
    /// writer rechecks both lanes on every wake and the run loop
    /// keeps notifying on every successful enqueue.
    pub(crate) fn notify(&self) -> bool {
        self.notify_tx.try_send(()).is_ok()
    }
}

/// Spawns the frame-writer thread owning `writer` and returns its
/// queues. The thread takes a queued data frame first, then serves
/// queued control ahead of it at every intact frame boundary: the
/// recheck after the take is unavoidable, so a control frame
/// arriving with or just after the data is still served first, and a
/// held data frame waits across further control writes until no
/// control remains queued. A data frame already being written is
/// never preempted: no control frame is ever injected mid-frame.
/// The writer parks on a cross-lane notify instead of polling,
/// so it wakes for a frame on either lane with no blind window and
/// burns no idle CPU. Scheduling note: cross-lane interleave is
/// decision-time, not trace-order; per-lane FIFO order and terminal
/// outcomes do not depend on thread timing. Every completion is
/// reported back on `events`. On a write failure the thread reports the transport
/// dead and stops: no further bytes are attempted and no wire
/// notification follows. When both queues disconnect the thread
/// reports [`WriterEvent::Exited`] after draining what was already
/// enqueued and stops; the run loop never joins it, so a blocked
/// transport cannot park shutdown — the process exit reclaims it, as
/// with the reader thread.
pub(crate) fn spawn_writer(
    writer: impl Write + Send + 'static,
    events: std::sync::mpsc::Sender<WriterEvent>,
) -> FrameSink {
    spawn_writer_with_hook(writer, events, None)
}

/// Spawn variant with a scheduling-staging hook: when `Some`, the
/// hook runs after every observed-empty control-channel check and
/// before the deferred-backlog load. It stages a feasible thread
/// schedule for the staged-transfer test (pause the writer there,
/// flush the deferred tail on the test thread, resume) and changes
/// no scheduling semantics: the hook observes nothing, decides
/// nothing, and only blocks the writer thread while the test stages
/// its transfer. `None` (every production call site) skips one
/// branch. The seam stays `pub(crate)`: scheduling-interleave
/// coverage for a priority invariant cannot be staged black-box, so
/// this narrow seam stands in for debugger-driven scheduling.
pub(crate) fn spawn_writer_with_hook(
    writer: impl Write + Send + 'static,
    events: std::sync::mpsc::Sender<WriterEvent>,
    stage_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
) -> FrameSink {
    // Queue depth bounds channel memory; the Outbox byte/count caps
    // bound admitted traffic first, so this only bites when the
    // writer stalls on a dead transport. The depth is small on
    // purpose: a stalled writer must surface as drain-wait expiry
    // after a bounded number of frames, not as unbounded queueing.
    // Separate depths per lane are the admission side of the
    // priority lane: data pressure can never refuse a control
    // enqueue, whatever the writer's pace.
    let (data_tx, data_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    let (ctrl_tx, ctrl_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    // Cross-lane wakeups: every successful enqueue posts one, so a
    // parked writer wakes for traffic on either lane. Unbounded so
    // a fast enqueuer never blocks on notifications; coalesced on
    // receipt since each wakeup rechecks both lanes.
    // Depth one: at most a single wakeup is ever retained, however
    // many frames enqueue while the writer is busy.
    let (notify_tx, notify_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let deferred_ctrl =
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let deferred_watch = deferred_ctrl.clone();
    std::thread::spawn(move || {
        let deferred_ctrl = deferred_watch;
        let stage_hook = stage_hook;
        let mut writer = writer;
        let mut data_closed = false;
        let mut ctrl_closed = false;
        // The run loop is gone once the notify senders drop: no more
        // flushes will arrive, so the deferred count can never fall
        // again. Ignore it from then on and drain what is already
        // queued instead of parking on a stale backlog.
        let mut senders_gone = false;
        // A data frame preempted by queued control waits here: every
        // newly reached intact boundary serves queued control before
        // a not-yet-started data frame, so control arriving during
        // another control frame's write is served first.
        let mut held: Option<Vec<u8>> = None;
        loop {
            if data_closed && ctrl_closed && held.is_none() {
                break;
            }
            // Take data first if any is queued; the control check
            // below decides whether it may be written yet. Taking
            // first makes the recheck unavoidable: every data write
            // is preceded by a queued-control check in the same
            // iteration, so no scheduling can bypass the boundary.
            if held.is_none() {
                match data_rx.try_recv() {
                    Ok(frame) => held = Some(frame),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        data_closed = true;
                    },
                    Err(std::sync::mpsc::TryRecvError::Empty) => {},
                }
            }
            // At every intact frame boundary, queued control goes
            // first: a held data frame waits while any control is
            // queued, and control arriving during a control write is
            // served next. A data frame already being written is
            // never preempted, and control arriving after this check
            // is served next.
            match ctrl_rx.try_recv() {
                Ok(cframe) => {
                    if emit(&mut writer, &events, Lane::Control, &cframe)
                        .is_err()
                    {
                        return;
                    }
                    continue;
                },
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    ctrl_closed = true;
                },
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Scheduling-staging seam (test hook only): a
                    // deferred-to-channel transfer landing here,
                    // before the backlog load below, is the handoff
                    // the pre-emit recheck closes.
                    if let Some(hook) = &stage_hook {
                        hook();
                    }
                },
            }
            // Admitted-but-unflushed control outranks held data:
            // park for the run loop's flush (which notifies on every
            // moved frame) instead of committing data ahead of it.
            // A parked writer with empty lanes takes the same path
            // with nothing held.
            if !senders_gone
                && (deferred_ctrl.load(std::sync::atomic::Ordering::Acquire)
                    > 0
                    || held.is_none())
            {
                match notify_rx.recv() {
                    Ok(()) => {
                        while notify_rx.try_recv().is_ok() {}
                        continue;
                    },
                    Err(_) => {
                        // Senders gone with the queues; the loop head
                        // observes each lane's own disconnect and
                        // drains what remains.
                        senders_gone = true;
                        continue;
                    },
                }
            }
            match ctrl_rx.try_recv() {
                Ok(cframe) => {
                    if emit(&mut writer, &events, Lane::Control, &cframe)
                        .is_err()
                    {
                        return;
                    }
                    continue;
                },
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    ctrl_closed = true;
                },
                Err(std::sync::mpsc::TryRecvError::Empty) => {},
            }
            if let Some(frame) = held.take()
                && emit(&mut writer, &events, Lane::Data, &frame).is_err()
            {
                return;
            }
        }
        let _ = events.send(WriterEvent::Exited);
    });
    FrameSink {
        data_tx,
        ctrl_tx,
        notify_tx,
        deferred_ctrl,
    }
}

/// Writes one frame plus its newline, flushes, and reports the
/// completion. Returns Err after reporting the transport dead, in
/// which case the caller stops without further attempts.
fn emit(
    writer: &mut (impl Write + ?Sized),
    events: &std::sync::mpsc::Sender<WriterEvent>,
    lane: Lane,
    frame: &[u8],
) -> Result<(), ()> {
    let len = frame.len();
    if write_one(writer, frame).is_err() {
        let _ = events.send(WriterEvent::Dead {
            context: "transport write failed".to_string(),
        });
        return Err(());
    }
    let _ = events.send(WriterEvent::Written { lane, len });
    Ok(())
}

/// Writes one frame plus its newline and flushes. The bound check
/// happens at enqueue time on the run loop against the operative
/// bound; the writer only moves bytes.
fn write_one(
    writer: &mut (impl Write + ?Sized),
    payload: &[u8],
) -> std::io::Result<()> {
    writer.write_all(payload)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

/// Writer lane tests: one test function by the inline-test rule.
#[cfg(test)]
mod writer_lane_tests {
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use super::{Lane, WriterEvent, spawn_writer};

    /// Gated transport: write() signals entry, then parks until the
    /// test opens the gate, then records the payload. Lets the test
    /// fill both lanes while the writer is parked, so admission
    /// behavior — not arrival timing — decides the assertions.
    struct GateWriter {
        gate: Arc<(Mutex<bool>, Condvar)>,
        entered: Arc<AtomicBool>,
        out: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl Write for GateWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.entered.store(true, Ordering::Release);
            let (lock, cvar) = &*self.gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cvar.wait(open).unwrap();
            }
            self.out.lock().unwrap().push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Admission isolation with boundary priority: with the writer
    /// parked, seventeen data frames saturate the data channel (one
    /// held in the parked write, sixteen queued) so an eighteenth
    /// data frame is refused at once — while two control frames still
    /// enqueue immediately. Data pressure never parks control at
    /// admission. Opening the gate finishes the in-progress data
    /// frame, then serves BOTH queued controls ahead of the remaining
    /// data: every newly reached intact boundary rechecks queued
    /// control, so a second control queued behind the first is not
    /// bypassed by held data. Per-lane completion accounting matches
    /// throughout. A shared FIFO would refuse the control frames on
    /// the full queue; serving held data before the second control
    /// would emit it third.
    #[test]
    fn flooded_data_never_parks_control_and_boundary_serves_it_first() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(AtomicBool::new(false));
        let out = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, event_rx) = std::sync::mpsc::channel::<WriterEvent>();
        let sink = spawn_writer(
            GateWriter {
                gate: gate.clone(),
                entered: entered.clone(),
                out: out.clone(),
            },
            event_tx,
        );
        let data =
            |n: u32| sink.data_tx.try_send(format!("DATA-{n}").into_bytes());
        data(1).expect("first data enqueues");
        // Wake the writer for the first frame: without a notify a
        // writer that parked before this send would never take it.
        sink.notify();
        // The writer holds DATA-1 in its parked write: only now fill
        // the lane behind it, so admission isolation — not timing —
        // is under test.
        let start = std::time::Instant::now();
        while !entered.load(Ordering::Acquire) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "writer must take the first frame"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        for n in 2..=17 {
            data(n).expect("data lane holds sixteen queued");
        }
        assert!(
            data(18).is_err(),
            "seventeenth queued data frame exceeds the lane bound"
        );
        sink.ctrl_tx
            .try_send(b"CTRL-1".to_vec())
            .expect("control isolated from the data flood");
        sink.ctrl_tx
            .try_send(b"CTRL-2".to_vec())
            .expect("second control isolated too");
        // Wake the parked writer for the queued burst (mirrors the
        // run loop, which notifies on every successful enqueue).
        for _ in 0..19 {
            sink.notify();
        }
        {
            let (lock, cvar) = &*gate;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }
        let mut order = Vec::new();
        for _ in 0..19 {
            match event_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("completions arrive")
            {
                WriterEvent::Written { lane, .. } => order.push(lane),
                WriterEvent::Dead { context } => {
                    panic!("transport must stay live: {context}")
                },
                WriterEvent::Exited => panic!("writer must not exit early"),
            }
        }
        let mut expected = vec![Lane::Data, Lane::Control, Lane::Control];
        expected.extend(std::iter::repeat_n(Lane::Data, 16));
        assert_eq!(
            order, expected,
            "in-progress data finishes, then every queued control preempts the flood"
        );
        // Each frame lands as payload plus newline writes; the
        // payloads alone must show seventeen data frames then the
        // control frame.
        let payloads = out.lock().unwrap();
        let frames: Vec<Vec<u8>> = payloads
            .iter()
            .filter(|chunk| chunk.as_slice() != b"\n")
            .cloned()
            .collect();
        assert_eq!(frames.len(), 19);
        assert_eq!(frames[0], b"DATA-1".to_vec());
        assert_eq!(frames[1], b"CTRL-1".to_vec());
        assert_eq!(frames[2], b"CTRL-2".to_vec());
        assert_eq!(frames[18], b"DATA-17".to_vec());
        drop(sink);
    }
}

/// Writer recheck tests: one test function by the inline-test rule.
#[cfg(test)]
mod writer_recheck_tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{Lane, WriterEvent, spawn_writer};

    /// Appending transport: every write lands immediately behind a
    /// lock. Fast enough that scheduling, not transport latency,
    /// decides every interleaving under test.
    #[derive(Clone)]
    struct VecWriter(Arc<Mutex<Vec<Vec<u8>>>>);

    impl Write for VecWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The take-then-recheck seam is deterministic: with the writer
    /// parked with empty lanes, a C1, C2, D burst resolves to
    /// control-first no matter where the writer is in its wait cycle,
    /// because both orders of observation converge — waking with the
    /// data frame rechecks queued control, and waking on the
    /// notification serves queued control at the loop head. Thirty
    /// sequential bursts on one hot writer sample the scheduling
    /// space; every round must emit C1, C2, D in lane order with
    /// matching payloads.
    #[test]
    fn parked_burst_serves_queued_control_before_taken_data() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, event_rx) = std::sync::mpsc::channel::<WriterEvent>();
        let sink = spawn_writer(VecWriter(out.clone()), event_tx);
        // No parking sleep: the single trailing notify per burst
        // orders every send before the writer's wake, so the
        // take-then-recheck path is exercised deterministically on
        // every round regardless of thread scheduling.
        for round in 0..30 {
            let tag = round.to_string();
            let item = |lane: &str| {
                let payload = format!("{lane}-{tag}").into_bytes();
                match lane {
                    "C" => sink.ctrl_tx.try_send(payload),
                    _ => sink.data_tx.try_send(payload),
                }
                .unwrap_or_else(|_| panic!("round {round}: {lane} enqueues"))
            };
            item("C");
            item("C");
            item("D");
            sink.notify();
            let mut order = Vec::new();
            for _ in 0..3 {
                match event_rx
                    .recv_timeout(Duration::from_secs(10))
                    .expect("completions arrive")
                {
                    WriterEvent::Written { lane, .. } => order.push(lane),
                    WriterEvent::Dead { context } => {
                        panic!("transport must stay live: {context}")
                    },
                    WriterEvent::Exited => {
                        panic!("writer must not exit early")
                    },
                }
            }
            assert_eq!(
                order,
                vec![Lane::Control, Lane::Control, Lane::Data],
                "round {round}: queued control preempts taken data"
            );
            let payloads = out.lock().unwrap();
            let frames: Vec<Vec<u8>> = payloads
                .iter()
                .filter(|chunk| chunk.as_slice() != b"\n")
                .cloned()
                .collect();
            let tail = &frames[frames.len().saturating_sub(3)..];
            assert_eq!(
                tail,
                &[
                    format!("C-{tag}").into_bytes(),
                    format!("C-{tag}").into_bytes(),
                    format!("D-{tag}").into_bytes()
                ],
                "round {round}: payloads follow the boundary"
            );
        }
        drop(sink);
    }
}

/// Writer control-only tests: one test function by the inline-test rule.
#[cfg(test)]
mod writer_control_only_tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{Lane, WriterEvent, spawn_writer};

    /// Appending transport for control-only traffic.
    struct VecWriter(Arc<Mutex<Vec<Vec<u8>>>>);

    impl Write for VecWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A parked writer serves control with an empty data lane: with
    /// no data ever enqueued, two control frames must still be
    /// written — the loop-head check, not the take-then-recheck,
    /// serves this path. Without the head check the writer would
    /// park on its empty data lane forever and this test would time
    /// out instead of completing.
    #[test]
    fn parked_writer_serves_control_with_empty_data_lane() {
        let out = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, event_rx) = std::sync::mpsc::channel::<WriterEvent>();
        let sink = spawn_writer(VecWriter(out.clone()), event_tx);
        sink.ctrl_tx.try_send(b"C1".to_vec()).expect("C1 enqueues");
        sink.notify();
        sink.ctrl_tx.try_send(b"C2".to_vec()).expect("C2 enqueues");
        sink.notify();
        let mut order = Vec::new();
        for _ in 0..2 {
            match event_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("completions arrive")
            {
                WriterEvent::Written { lane, .. } => order.push(lane),
                WriterEvent::Dead { context } => {
                    panic!("transport must stay live: {context}")
                },
                WriterEvent::Exited => panic!("writer must not exit early"),
            }
        }
        assert_eq!(order, vec![Lane::Control, Lane::Control]);
        let payloads = out.lock().unwrap();
        let frames: Vec<Vec<u8>> = payloads
            .iter()
            .filter(|chunk| chunk.as_slice() != b"\n")
            .cloned()
            .collect();
        assert_eq!(frames, vec![b"C1".to_vec(), b"C2".to_vec()]);
        drop(sink);
    }
}

/// Writer slow-drain tests: one test function by the inline-test
/// rule.
#[cfg(test)]
mod writer_slow_tests {
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::{Lane, WriterEvent, spawn_writer};

    /// Slow transport with write-entry signalling: each write parks a
    /// fixed drain interval first, so control arriving mid-write is
    /// staged deterministically by observing entry.
    struct SlowWriter {
        entered: Arc<AtomicBool>,
        delay_ms: u64,
        out: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    }

    impl Write for SlowWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.entered.store(true, Ordering::Release);
            std::thread::sleep(Duration::from_millis(self.delay_ms));
            self.out.lock().unwrap().push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Control arriving during a control write still precedes held
    /// data: C1's slow write is observed via its entry flag, C3 is
    /// enqueued mid-write, and the writer must emit C2 (older queued)
    /// then C3 before the data frame taken before either control
    /// arrived. The recheck chose C1 over the taken D1; the loop head
    /// then drains C2 and C3 in enqueue order.
    #[test]
    fn control_arriving_during_control_write_still_precedes_data() {
        let entered = Arc::new(AtomicBool::new(false));
        let out = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (event_tx, event_rx) = std::sync::mpsc::channel::<WriterEvent>();
        let sink = spawn_writer(
            SlowWriter {
                entered: entered.clone(),
                delay_ms: 50,
                out: out.clone(),
            },
            event_tx,
        );
        // No parking sleep is needed: the trailing notify orders
        // the whole burst before the writer's wake, and the entered
        // flag below proves C1's write started (hence the recheck
        // chose it) before C3 lands.
        sink.ctrl_tx.try_send(b"C1".to_vec()).expect("C1 enqueues");
        sink.ctrl_tx.try_send(b"C2".to_vec()).expect("C2 enqueues");
        sink.data_tx.try_send(b"D1".to_vec()).expect("D1 enqueues");
        sink.notify();
        // Wait until C1's write has started (the recheck already
        // Wait until C1's write has started (the recheck already
        // preferred it over the taken data), then land C3 mid-write.
        let start = std::time::Instant::now();
        while !entered.load(Ordering::Acquire) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "C1 write must start"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(10));
        sink.ctrl_tx.try_send(b"C3".to_vec()).expect("C3 enqueues");
        sink.notify();
        let mut order = Vec::new();
        for _ in 0..4 {
            match event_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("completions arrive")
            {
                WriterEvent::Written { lane, .. } => order.push(lane),
                WriterEvent::Dead { context } => {
                    panic!("transport must stay live: {context}")
                },
                WriterEvent::Exited => panic!("writer must not exit early"),
            }
        }
        assert_eq!(
            order,
            vec![Lane::Control, Lane::Control, Lane::Control, Lane::Data],
            "C1, older C2, mid-write C3, then the taken data"
        );
        let payloads = out.lock().unwrap();
        let frames: Vec<Vec<u8>> = payloads
            .iter()
            .filter(|chunk| chunk.as_slice() != b"\n")
            .cloned()
            .collect();
        assert_eq!(
            frames,
            vec![
                b"C1".to_vec(),
                b"C2".to_vec(),
                b"C3".to_vec(),
                b"D1".to_vec()
            ]
        );
        drop(sink);
    }
}

/// Notify-coalescing tests: one test function by the inline-test
/// rule.
#[cfg(test)]
mod writer_notify_tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{Lane, WriterEvent, spawn_writer};

    /// Gated transport for parking the writer mid-write while
    /// notifications pile up behind it.
    struct GateWriter {
        gate: Arc<(Mutex<bool>, std::sync::Condvar)>,
        entered: Arc<AtomicBool>,
        out: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl Write for GateWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.entered.store(true, Ordering::Release);
            let (lock, cvar) = &*self.gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cvar.wait(open).unwrap();
            }
            self.out.lock().unwrap().push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Notifications coalesce instead of accumulating: with the
    /// writer parked inside a write, two hundred notifies retain at
    /// most one pending wakeup (an unbounded channel would retain
    /// all two hundred). The single retained wakeup still wakes the
    /// writer: opening the gate and sending one more frame drains it.
    #[test]
    fn notify_coalesces_under_parked_writer() {
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let entered = Arc::new(AtomicBool::new(false));
        let out = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, event_rx) = std::sync::mpsc::channel::<WriterEvent>();
        let sink = spawn_writer(
            GateWriter {
                gate: gate.clone(),
                entered: entered.clone(),
                out: out.clone(),
            },
            event_tx,
        );
        sink.data_tx.try_send(b"D0".to_vec()).expect("D0 enqueues");
        sink.notify();
        let start = std::time::Instant::now();
        while !entered.load(Ordering::Acquire) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "writer must take D0"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // The writer is parked inside D0's write and never touches
        // the notify channel: at most one wakeup can be retained no
        // matter how many notifies arrive.
        let mut accepted = 0;
        for _ in 0..200 {
            if sink.notify() {
                accepted += 1;
            }
        }
        assert!(
            accepted <= 1,
            "coalesced wakeups stay bounded: {accepted} retained"
        );
        // The retained (or fresh) wakeup path still works: opening
        // the gate and sending one more frame completes it.
        {
            let (lock, cvar) = &*gate;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }
        sink.data_tx.try_send(b"D1".to_vec()).expect("D1 enqueues");
        sink.notify();
        let mut lanes = Vec::new();
        for _ in 0..2 {
            match event_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("completions arrive")
            {
                WriterEvent::Written { lane, .. } => lanes.push(lane),
                WriterEvent::Dead { context } => {
                    panic!("transport must stay live: {context}")
                },
                WriterEvent::Exited => panic!("writer must not exit early"),
            }
        }
        assert_eq!(lanes, vec![Lane::Data, Lane::Data]);
        drop(sink);
    }
}
