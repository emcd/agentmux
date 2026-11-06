//! Outbound write path for the reference adapter: lane-checked
//! admission with a bounded drain wait and no run-loop parking.
//! Data frames count against the peer's receive cap; control frames
//! ride the priority lane exempt from data caps within their own
//! bound. Admission reserves Outbox usage first, so every queued or
//! deferred byte is bounded by the negotiated caps. A write that
//! cannot enqueue at once is DEFERRED, never retried in place: the
//! run loop stays responsive to inbound fence, cancel, terminate,
//! and ping traffic while the lane is full, and the deferred queues
//! drain in enqueue order at every loop turn. Only a full drain wait
//! with zero completed writes fails the session (via the loop-top
//! watchdog), never a single blocked send.

use std::sync::atomic::Ordering;

use serde_json::Value;

use super::Adapter;
use super::writer::{Lane, WriterEvent};

impl<'a> Adapter<'a> {
    pub(crate) fn write_value(&mut self, value: &Value) -> Result<(), ()> {
        let bytes = serde_json::to_vec(value).map_err(|_| ())?;
        self.drain_writer_events();
        if self.outbox.push_data(bytes.len()).is_err() {
            self.record("outbox_full", "response refused at queue cap");
            self.failed = true;
            return Err(());
        }
        // Operative bound is the negotiated maximum; the bootstrap
        // ceiling applies while frame_bound still holds it.
        let bound = self.frame_bound.load(Ordering::Relaxed);
        if bytes.len() > bound {
            self.outbox.release_data(bytes.len());
            self.record("transport_write_failed", "frame exceeds the bound");
            self.failed = true;
            return Err(());
        }
        let result = self.enqueue(Lane::Data, bytes);
        if result.is_ok() {
            self.note_queued();
        }
        result
    }

    /// Writes a control-lane frame (lifecycle events, ping replies,
    /// submit/persist/choice terminal outcomes): exempt from data
    /// caps within its own negotiated control bound. Admission
    /// failure escalates: the session terminalizes rather than
    /// dropping or silently queueing control traffic. A full control
    /// channel defers like data; the run loop keeps dispatching
    /// rather than parking in the send.
    pub(crate) fn write_control(&mut self, value: &Value) -> Result<(), ()> {
        let bytes = serde_json::to_vec(value).map_err(|_| ())?;
        self.drain_writer_events();
        if self.outbox.push_control().is_err() {
            self.record(
                "control_lane_full",
                "control admission escalates to terminalization",
            );
            self.failed = true;
            return Err(());
        }
        let bound = self.frame_bound.load(Ordering::Relaxed);
        if bytes.len() > bound {
            self.outbox.release_control();
            self.record("control_write_failed", "frame exceeds the bound");
            self.failed = true;
            return Err(());
        }
        let result = self.enqueue(Lane::Control, bytes);
        if result.is_ok() {
            self.note_queued();
        }
        result
    }

    /// Enqueues one bound-checked frame on its lane WITHOUT blocking:
    /// a full lane defers the frame to the per-lane deferred queue
    /// (still counted against the Outbox reservation, so deferred
    /// memory stays bounded by the negotiated caps) and reports
    /// success — the run loop flushes deferred frames every turn and
    /// never parks inside a send. When the lane already holds
    /// deferred frames the new one appends behind them no matter
    /// what the channel currently holds: skipping the queue on a
    /// transiently free slot would reorder same-lane traffic. A
    /// disconnected queue means the writer already reported the
    /// transport dead (no wire reply is required on a dead channel):
    /// usage is released and the attempt terminalizes locally. In
    /// lockstep replay each direct send additionally waits for its
    /// own completion (see quiesce), which is what makes replayed
    /// wire order exactly the enqueue order.
    fn enqueue(&mut self, lane: Lane, bytes: Vec<u8>) -> Result<(), ()> {
        let len = bytes.len();
        // Deferred backlog first: a lane holding unflushed frames
        // keeps exact enqueue order by appending behind them, even
        // when the channel momentarily has room again.
        match lane {
            Lane::Data if !self.deferred_data.is_empty() => {
                self.deferred_data.push_back(bytes);
                return Ok(());
            },
            Lane::Control if !self.deferred_ctrl.is_empty() => {
                // Increment first so the count never understates the
                // queue to a concurrent writer take.
                if let Some(queues) = &self.queues {
                    queues.deferred_ctrl.fetch_add(1, Ordering::Release);
                }
                self.deferred_ctrl.push_back(bytes);
                return Ok(());
            },
            _ => {},
        }
        // Clone the lane sender so the rest of this function owns no
        // queue borrow and may freely mutate accounting below.
        let tx = match self.queues.as_ref() {
            Some(queues) => match lane {
                Lane::Data => queues.data_tx.clone(),
                Lane::Control => queues.ctrl_tx.clone(),
            },
            None => {
                self.record("transport_dead", "writer is gone");
                self.failed = true;
                return Err(());
            },
        };
        // Separate bounded channels per lane: control never waits
        // behind a full data queue, and data never waits behind a
        // full control queue. Deferred frames preserve exact enqueue
        // order per lane; the writer serves queued control first at
        // every intact frame boundary.
        match tx.try_send(bytes) {
            Ok(()) => {
                if let Some(queues) = &self.queues {
                    queues.notify();
                }
                if self.lockstep_replay {
                    // Replay seam: the next enqueue starts only once
                    // this frame is on the wire, so cross-lane order
                    // is exactly enqueue order under every schedule.
                    self.quiesce();
                }
                Ok(())
            },
            Err(std::sync::mpsc::TrySendError::Full(frame)) => {
                match lane {
                    Lane::Data => self.deferred_data.push_back(frame),
                    Lane::Control => {
                        if let Some(queues) = &self.queues {
                            queues
                                .deferred_ctrl
                                .fetch_add(1, Ordering::Release);
                        }
                        self.deferred_ctrl.push_back(frame);
                    },
                }
                Ok(())
            },
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                match lane {
                    Lane::Data => self.outbox.release_data(len),
                    Lane::Control => self.outbox.release_control(),
                }
                self.record("transport_dead", "writer reported a dead peer");
                self.failed = true;
                Err(())
            },
        }
    }

    /// Blocks until every outstanding byte is acknowledged, so the
    /// next enqueue starts with an idle writer and the wire order is
    /// exactly the enqueue order. Only used in lockstep replay;
    /// requires a draining writer (a parked transport stalls here
    /// exactly as it stalls a live drain — replay traces must drain).
    fn quiesce(&mut self) {
        while !self.outbox.is_empty() && !self.failed && !self.writer_exited {
            match self.writer_events.recv() {
                Ok(WriterEvent::Written { lane, len }) => match lane {
                    Lane::Data => self.outbox.release_data(len),
                    Lane::Control => self.outbox.release_control(),
                },
                Ok(WriterEvent::Dead { context }) => {
                    self.record("transport_dead", &context);
                    self.failed = true;
                },
                Ok(WriterEvent::Exited) => {
                    self.writer_exited = true;
                },
                Err(_) => {
                    self.failed = true;
                },
            }
        }
    }

    /// Flushes deferred frames in enqueue order, control lane first:
    /// queued control preempts queued data at this boundary, while
    /// each lane keeps its own order. Stops a lane at the first full
    /// channel; a disconnected queue drops the lane's remainder with
    /// usage released and marks the transport failed.
    pub(crate) fn flush_deferred(&mut self) {
        for lane in [Lane::Control, Lane::Data] {
            loop {
                let front = match lane {
                    Lane::Data => self.deferred_data.front(),
                    Lane::Control => self.deferred_ctrl.front(),
                };
                if front.is_none() {
                    break;
                }
                let Some(queues) = &self.queues else {
                    self.drop_deferred(lane, "writer is gone");
                    break;
                };
                let tx = match lane {
                    Lane::Data => &queues.data_tx,
                    Lane::Control => &queues.ctrl_tx,
                };
                // Pop-then-send: a Full return puts the frame back at
                // the head, preserving exact lane order.
                let frame = match lane {
                    Lane::Data => {
                        self.deferred_data.pop_front().expect("head seen")
                    },
                    Lane::Control => {
                        self.deferred_ctrl.pop_front().expect("head seen")
                    },
                };
                match tx.try_send(frame) {
                    Ok(()) => {
                        // A moved control frame leaves the deferred
                        // set: publish the shorter queue so the
                        // writer's view never lags behind reality.
                        if matches!(lane, Lane::Control) {
                            queues
                                .deferred_ctrl
                                .fetch_sub(1, Ordering::AcqRel);
                        }
                        // Wake a writer parked with empty lanes: the
                        // moved frame is new work either way.
                        queues.notify();
                    },
                    Err(std::sync::mpsc::TrySendError::Full(returned)) => {
                        match lane {
                            Lane::Data => {
                                self.deferred_data.push_front(returned);
                            },
                            Lane::Control => {
                                self.deferred_ctrl.push_front(returned);
                            },
                        }
                        break;
                    },
                    Err(std::sync::mpsc::TrySendError::Disconnected(
                        frame,
                    )) => {
                        // The popped head can never send: release its
                        // reservation with the remainder below.
                        match lane {
                            Lane::Data => {
                                self.outbox.release_data(frame.len());
                            },
                            Lane::Control => self.outbox.release_control(),
                        }
                        self.drop_deferred(
                            lane,
                            "writer reported a dead peer",
                        );
                        break;
                    },
                }
            }
        }
        if self.lockstep_replay {
            // Lockstep replay: flushed frames complete before the
            // run loop proceeds, keeping even flooded replays byte-exact.
            self.quiesce();
        }
    }

    /// Drops a lane's deferred queue after transport death, releasing
    /// every reserved byte so accounting stays exact, and records the
    /// typed cause once. The writer's view of the control backlog is
    /// reset to match: nothing remains unflushed.
    fn drop_deferred(&mut self, lane: Lane, detail: &str) {
        let (count, bytes) = match lane {
            Lane::Data => {
                let frames = std::mem::take(&mut self.deferred_data);
                let bytes = frames.iter().map(Vec::len).sum();
                (frames.len(), bytes)
            },
            Lane::Control => {
                let frames = std::mem::take(&mut self.deferred_ctrl);
                (frames.len(), 0)
            },
        };
        match lane {
            Lane::Data => self.outbox.release_data(bytes),
            Lane::Control => {
                for _ in 0..count {
                    self.outbox.release_control();
                }
                if let Some(queues) = &self.queues {
                    queues.deferred_ctrl.store(0, Ordering::Release);
                }
            },
        }
        let _ = count;
        self.record("transport_dead", detail);
        self.failed = true;
    }
}

/// Lockstep tests: one test function by the inline-test rule.
#[cfg(test)]
mod lockstep_tests {
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::super::handshake::{LimitsProposal, VersionOffer};
    use super::super::lifecycle::ManualClock;
    use super::super::script::parse_script;
    use super::super::writer::spawn_writer;
    use super::super::{Adapter, AdapterConfig};
    use serde_json::json;

    fn test_config() -> AdapterConfig {
        let mut latency = HashMap::from([("default".to_string(), 5_000_u64)]);
        latency.insert("text".to_string(), 5_000);
        AdapterConfig {
            executable: "agentmux-adapter".to_string(),
            profile: "generic".to_string(),
            target_session: "seat-1".to_string(),
            offer: VersionOffer {
                major: 2,
                min_minor: 0,
                max_minor: 1,
            },
            proposal: LimitsProposal {
                max_frame_bytes: 65_536,
                queue_tx_bytes: 16_777_216,
                queue_rx_bytes: 16_777_216,
                max_control_pending: 32,
                max_events_per_second: 100,
                window_seconds: 1,
                max_submits_in_flight: 64,
                max_choices_in_flight: 16,
                max_looks_in_flight: 8,
                drain_wait_ms: 250,
                latency_declarations: latency,
            },
            script: parse_script("").expect("empty script parses"),
            handshake_timeout_ms: 30_000,
            liveness_timeout_ms: 60_000,
        }
    }

    /// A fixed 30ms transport write.
    struct SlowWriter {
        out: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for SlowWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            std::thread::sleep(Duration::from_millis(30));
            self.out.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// In lockstep replay, a direct enqueue returns only after its
    /// frame is on the wire: with a 30ms transport, `write_value`
    /// takes at least 25ms and the outbox is empty on return. The
    /// bound is one-sided on purpose — scheduling can only add
    /// latency, never remove the quiesced write.
    #[test]
    fn lockstep_enqueue_waits_for_completion() {
        let config = test_config();
        let clock = ManualClock::new(1_000);
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let queues = spawn_writer(
            SlowWriter {
                out: Arc::new(Mutex::new(Vec::new())),
            },
            event_tx,
        );
        let mut sink = Vec::new();
        let mut adapter =
            Adapter::new(&config, &clock, queues, event_rx, &mut sink);
        adapter.lockstep_replay = true;
        let start = Instant::now();
        adapter
            .write_value(&json!({"jsonrpc": "2.0", "id": "1"}))
            .expect(" admitted");
        assert!(
            start.elapsed() >= Duration::from_millis(25),
            "quiesce waits out the write"
        );
        assert!(
            adapter.outbox.is_empty(),
            "completion is accounted before return"
        );
    }
}

/// Deferred-order tests: one test function by the inline-test rule.
#[cfg(test)]
mod deferred_tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use super::super::handshake::{LimitsProposal, VersionOffer};
    use super::super::lifecycle::ManualClock;
    use super::super::script::parse_script;
    use super::super::writer::{spawn_writer, spawn_writer_with_hook};
    use super::super::{Adapter, AdapterConfig};
    use serde_json::json;

    fn test_config() -> AdapterConfig {
        let mut latency = HashMap::from([("default".to_string(), 5_000_u64)]);
        latency.insert("text".to_string(), 5_000);
        AdapterConfig {
            executable: "agentmux-adapter".to_string(),
            profile: "generic".to_string(),
            target_session: "seat-1".to_string(),
            offer: VersionOffer {
                major: 2,
                min_minor: 0,
                max_minor: 1,
            },
            proposal: LimitsProposal {
                max_frame_bytes: 65_536,
                queue_tx_bytes: 16_777_216,
                queue_rx_bytes: 16_777_216,
                max_control_pending: 32,
                max_events_per_second: 100,
                window_seconds: 1,
                max_submits_in_flight: 64,
                max_choices_in_flight: 16,
                max_looks_in_flight: 8,
                drain_wait_ms: 250,
                latency_declarations: latency,
            },
            script: parse_script("").expect("empty script parses"),
            handshake_timeout_ms: 30_000,
            liveness_timeout_ms: 60_000,
        }
    }

    /// Counting gate: each write consumes one permit, then parks
    /// until the test adds more. Lets the test stage exact channel
    /// occupancy — parked mid-write with N queued behind — before
    /// releasing the writer a step at a time.
    struct GateWriter {
        permits: Arc<(Mutex<usize>, Condvar)>,
        entered: Arc<AtomicUsize>,
        out: Arc<Mutex<Vec<Vec<u8>>>>,
    }

    impl std::io::Write for GateWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.entered.fetch_add(1, Ordering::Release);
            let (lock, cvar) = &*self.permits;
            let mut open = lock.lock().unwrap();
            while *open == 0 {
                open = cvar.wait(open).unwrap();
            }
            *open -= 1;
            self.out.lock().unwrap().push(bytes.to_vec());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn open_gate(gate: &Arc<(Mutex<usize>, Condvar)>, permits: usize) {
        let (lock, cvar) = &**gate;
        *lock.lock().unwrap() += permits;
        cvar.notify_all();
    }

    fn wait_for(entered: &Arc<AtomicUsize>, count: usize, what: &str) {
        let start = std::time::Instant::now();
        while entered.load(Ordering::Acquire) < count {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{what} must start"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A new enqueue appends behind a nonempty same-lane deferred
    /// backlog even when the channel momentarily has room: D0 is
    /// held in the write, D1..D16 fill the sixteen-deep channel,
    /// D17 defers, D1 completes and frees one slot, and only then
    /// D18 arrives — it must queue behind D17, not jump it.
    /// Skipping the backlog on a transiently free slot would emit
    /// [..., D16, D18, D17].
    #[test]
    fn deferred_lane_keeps_fifo_across_free_slots() {
        let config = test_config();
        let clock = ManualClock::new(1_000);
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let gate = Arc::new((Mutex::new(0_usize), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let out = Arc::new(Mutex::new(Vec::new()));
        let mut sink: Vec<super::super::choice::DiagnosticEntry> = Vec::new();
        let mut adapter = Adapter::new(
            &config,
            &clock,
            spawn_writer(
                GateWriter {
                    permits: gate.clone(),
                    entered: entered.clone(),
                    out: out.clone(),
                },
                event_tx,
            ),
            event_rx,
            &mut sink,
        );
        let send = |adapter: &mut Adapter<'_>, n: u32| {
            let frame = format!("D{n}").into_bytes();
            adapter
                .enqueue(super::super::writer::Lane::Data, frame)
                .expect("admitted");
            // The run loop notifies on every successful enqueue.
            if let Some(queues) = &adapter.queues {
                queues.notify();
            }
        };
        send(&mut adapter, 0);
        wait_for(&entered, 1, "D0 write");
        for n in 1..=16 {
            send(&mut adapter, n);
        }
        // Channel full (16 queued behind the held D0); this one
        // must wait its turn in the deferred queue.
        send(&mut adapter, 17);
        assert_eq!(adapter.deferred_data.len(), 1);
        // Free exactly one slot by completing D0 and starting D1.
        // Each frame costs two writes (payload plus newline), so the
        // third write start is D1's payload: D1 is taken and parked,
        // and the channel holds exactly fifteen behind it.
        open_gate(&gate, 2);
        wait_for(&entered, 3, "D1 write");
        // A fresh enqueue now sees room in the channel but a
        // nonempty backlog: it must append behind D17.
        send(&mut adapter, 18);
        assert_eq!(adapter.deferred_data.len(), 2);
        open_gate(&gate, 100);
        // Drain completions until all nineteen frames land; the
        // writer is live, so a stall here means lost wakeups, not
        // backpressure. Each turn flushes the deferred backlog
        // first, exactly like the run loop does.
        let start = std::time::Instant::now();
        let frames: Vec<Vec<u8>> = loop {
            adapter.flush_deferred();
            adapter.drain_writer_events();
            let payloads = out.lock().unwrap();
            let frames: Vec<Vec<u8>> = payloads
                .iter()
                .filter(|chunk| chunk.as_slice() != b"\n")
                .cloned()
                .collect();
            if frames.len() >= 19 {
                break frames;
            }
            drop(payloads);
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "writer must drain everything"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        drop(adapter);
        let expected: Vec<Vec<u8>> =
            (0..=18).map(|n| format!("D{n}").into_bytes()).collect();
        assert_eq!(frames, expected, "deferred D17 precedes D18");
    }

    /// Deferred control stays ahead of already-queued data: the
    /// control allowance (32) exceeds the sixteen-deep channel, so
    /// twenty admitted controls leave a genuine four-frame deferred
    /// tail while data is already held and queued. The writer drains
    /// every channel-visible frame between run-loop turns, then must
    /// park its held data until the deferred tail flushes — ready
    /// control includes admitted-but-unflushed control. Removing only
    /// the deferred-visibility guard emits D1 ahead of C17..C20 and
    /// fails the order assertion (eighteen payloads instead of
    /// seventeen at the park point, D1 before the tail at the end).
    #[test]
    fn deferred_control_tail_precedes_held_data() {
        let config = test_config();
        let clock = ManualClock::new(1_000);
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let gate = Arc::new((Mutex::new(0_usize), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let out = Arc::new(Mutex::new(Vec::new()));
        let mut sink: Vec<super::super::choice::DiagnosticEntry> = Vec::new();
        let mut adapter = Adapter::new(
            &config,
            &clock,
            spawn_writer(
                GateWriter {
                    permits: gate.clone(),
                    entered: entered.clone(),
                    out: out.clone(),
                },
                event_tx,
            ),
            event_rx,
            &mut sink,
        );
        let data_frame = |n: u32| json!({"data": n});
        let ctrl_frame = |n: u32| json!({"control": n});
        adapter.write_value(&data_frame(0)).expect("D0 admitted");
        wait_for(&entered, 1, "D0 write");
        for n in 1..=4 {
            adapter.write_value(&data_frame(n)).expect("data queues");
        }
        for n in 1..=16 {
            adapter
                .write_control(&ctrl_frame(n))
                .expect("control queues");
        }
        // Allowance 32 admits all twenty controls; the sixteen-deep
        // channel holds the first sixteen, so the last four wait in
        // the deferred queue: a genuine tail, not a cap refusal.
        for n in 17..=20 {
            adapter
                .write_control(&ctrl_frame(n))
                .expect("tail admitted");
        }
        assert_eq!(adapter.deferred_ctrl.len(), 4, "four controls defer");
        assert_eq!(
            adapter
                .queues
                .as_ref()
                .expect("writer live")
                .deferred_ctrl
                .load(Ordering::Acquire),
            4,
            "the writer sees the deferred tail"
        );
        // Drain the writer between run-loop turns without flushing:
        // every channel-visible frame (D0 plus C1..C16) can complete,
        // but the held data must wait for the deferred tail.
        open_gate(&gate, 100);
        let start = std::time::Instant::now();
        loop {
            let payloads = out.lock().unwrap();
            let count = payloads
                .iter()
                .filter(|chunk| chunk.as_slice() != b"\n")
                .count();
            if count >= 17 {
                break;
            }
            drop(payloads);
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "writer must drain the visible lanes"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        // Settle: a writer blind to the deferred tail would take and
        // emit D1 now (permits are open); the guarded writer parks.
        let parked = entered.load(Ordering::Acquire);
        let settle = std::time::Instant::now();
        while settle.elapsed() < Duration::from_millis(300) {
            assert_eq!(
                entered.load(Ordering::Acquire),
                parked,
                "held data waits for the deferred tail"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let payloads = out.lock().unwrap();
        let frames: Vec<Vec<u8>> = payloads
            .iter()
            .filter(|chunk| chunk.as_slice() != b"\n")
            .cloned()
            .collect();
        drop(payloads);
        assert_eq!(frames.len(), 17, "no data jumps the deferred tail");
        // Run-loop turns from here: each turn flushes deferred
        // control first, exactly like the live loop.
        let start = std::time::Instant::now();
        let frames: Vec<Vec<u8>> = loop {
            adapter.flush_deferred();
            adapter.drain_writer_events();
            let payloads = out.lock().unwrap();
            let frames: Vec<Vec<u8>> = payloads
                .iter()
                .filter(|chunk| chunk.as_slice() != b"\n")
                .cloned()
                .collect();
            if frames.len() >= 25 {
                break frames;
            }
            drop(payloads);
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "writer must drain everything"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        drop(adapter);
        let mut expected = vec![serde_json::to_vec(&data_frame(0)).unwrap()];
        for n in 1..=20 {
            expected.push(serde_json::to_vec(&ctrl_frame(n)).unwrap());
        }
        for n in 1..=4 {
            expected.push(serde_json::to_vec(&data_frame(n)).unwrap());
        }
        assert_eq!(
            frames, expected,
            "D0 finishes in progress, then every ready control, then held data"
        );
    }

    /// Deferred-to-channel transfer during the priority observation
    /// still serves the tail first: the writer is paused after an
    /// observed-empty control-channel check and before its backlog
    /// load (the staging hook stands in for debugger-driven
    /// scheduling and changes no scheduling semantics), the test
    /// thread flushes C17..C20 and publishes count 0, then resumes
    /// the writer. The pre-emit coherence recheck must serve the
    /// transferred tail before the held D1. Without that recheck the
    /// writer loads 0, skips parking, and commits D1 first — the
    /// staged trace fails with D1 ahead of C17. The hook is
    /// per-writer-instance (no shared staging state across tests),
    /// armed only after the fixture is fully admitted while the
    /// writer is frozen in its gated transport (so only the intended
    /// post-drain observation can rendezvous), fires at most once,
    /// and clears on drop even on panic.
    #[test]
    fn deferred_transfer_during_observation_serves_tail_first() {
        struct Disarm<'a> {
            flag: &'a AtomicBool,
        }
        impl Drop for Disarm<'_> {
            fn drop(&mut self) {
                self.flag.store(false, Ordering::Release);
            }
        }
        let config = test_config();
        let clock = ManualClock::new(1_000);
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let gate = Arc::new((Mutex::new(0_usize), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let out = Arc::new(Mutex::new(Vec::new()));
        let armed = Arc::new(AtomicBool::new(false));
        let fired = Arc::new(AtomicBool::new(false));
        let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
        let resume_rx = Mutex::new(resume_rx);
        let hook = {
            let armed = armed.clone();
            let fired = fired.clone();
            std::sync::Arc::new(move || {
                if !armed.load(Ordering::Acquire)
                    || fired.swap(true, Ordering::AcqRel)
                {
                    return;
                }
                let _ = paused_tx.send(());
                // Fail-open: a test-side panic before resume must not
                // wedge the writer past the test timeout.
                let _ = resume_rx
                    .lock()
                    .expect("resume mutex")
                    .recv_timeout(Duration::from_secs(10));
            })
        };
        let mut sink: Vec<super::super::choice::DiagnosticEntry> = Vec::new();
        let mut adapter = Adapter::new(
            &config,
            &clock,
            spawn_writer_with_hook(
                GateWriter {
                    permits: gate.clone(),
                    entered: entered.clone(),
                    out: out.clone(),
                },
                event_tx,
                Some(hook),
            ),
            event_rx,
            &mut sink,
        );
        let _disarm = Disarm { flag: &armed };
        let data_frame = |n: u32| json!({"data": n});
        let ctrl_frame = |n: u32| json!({"control": n});
        // The writer is frozen in its gated transport (zero permits)
        // from its first write, so nothing drains before arming:
        // only the intended post-drain observation rendezvouses. The
        // entry barrier establishes the in-progress D0 precondition
        // rather than inferring it from zero permits: without it a
        // late-starting writer would correctly serve C1..C16 before
        // its held D0 and reach the hook with sixteen payloads.
        adapter.write_value(&data_frame(0)).expect("D0 admitted");
        wait_for(&entered, 1, "D0 write");
        for n in 1..=4 {
            adapter.write_value(&data_frame(n)).expect("data queues");
        }
        for n in 1..=16 {
            adapter
                .write_control(&ctrl_frame(n))
                .expect("control queues");
        }
        for n in 17..=20 {
            adapter
                .write_control(&ctrl_frame(n))
                .expect("tail admitted");
        }
        assert_eq!(adapter.deferred_ctrl.len(), 4, "four controls defer");
        armed.store(true, Ordering::Release);
        open_gate(&gate, 100);
        paused_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("writer must reach the staged observation");
        // Seventeen payloads visible, tail still deferred: the writer
        // observed an empty control channel with count 4.
        let payloads = out.lock().unwrap();
        let staged: Vec<Vec<u8>> = payloads
            .iter()
            .filter(|chunk| chunk.as_slice() != b"\n")
            .cloned()
            .collect();
        drop(payloads);
        assert_eq!(staged.len(), 17, "visible lanes drained, tail pending");
        assert_eq!(adapter.deferred_ctrl.len(), 4, "tail still deferred");
        // The transfer under test: the whole tail moves to the
        // channel and the count publishes 0 while the writer is
        // paused between its channel check and its backlog load.
        adapter.flush_deferred();
        adapter.drain_writer_events();
        assert_eq!(
            adapter
                .queues
                .as_ref()
                .expect("writer live")
                .deferred_ctrl
                .load(Ordering::Acquire),
            0,
            "transferred tail publishes zero backlog"
        );
        resume_tx.send(()).expect("writer resumes");
        let start = std::time::Instant::now();
        let frames: Vec<Vec<u8>> = loop {
            adapter.flush_deferred();
            adapter.drain_writer_events();
            let payloads = out.lock().unwrap();
            let frames: Vec<Vec<u8>> = payloads
                .iter()
                .filter(|chunk| chunk.as_slice() != b"\n")
                .cloned()
                .collect();
            if frames.len() >= 25 {
                break frames;
            }
            drop(payloads);
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "writer must drain everything"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        drop(adapter);
        let mut expected = vec![serde_json::to_vec(&data_frame(0)).unwrap()];
        for n in 1..=20 {
            expected.push(serde_json::to_vec(&ctrl_frame(n)).unwrap());
        }
        for n in 1..=4 {
            expected.push(serde_json::to_vec(&data_frame(n)).unwrap());
        }
        assert_eq!(
            frames, expected,
            "transferred tail precedes held data despite the mid-observation move"
        );
    }
}
