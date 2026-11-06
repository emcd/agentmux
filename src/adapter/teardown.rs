//! Teardown and drain supervision for the reference adapter: the
//! stuck-writer watchdog with progress-aware refresh, the bounded
//! shutdown drain, and local terminalization. Teardown writes nothing
//! on a dead transport; held state retires silently while the
//! diagnostic sink records the outcome.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::json;

use super::Adapter;
use super::EXIT_CLEAN;
use super::binding::{CallKind, Direction};
use super::look;
use super::writer::{Lane, WriterEvent};

impl<'a> Adapter<'a> {
    /// Drains completed writer reports into queue accounting without
    /// blocking. A dead transport marks the session failed so the
    /// run loop terminalizes locally rather than answering over it.
    /// Clearing the queue disarms the drain watchdog; completed
    /// writes refresh it while bytes remain, so steady progress —
    /// however slow — never trips the wait. Only a full drain wait
    /// with zero completions marks the transport dead. This is the
    /// progress-aware reading of the frozen bounded drain wait: the
    /// bound limits how long a write may stay blocked, not how slowly
    /// a live transport may drain.
    pub(crate) fn drain_writer_events(&mut self) {
        while let Ok(event) = self.writer_events.try_recv() {
            match event {
                WriterEvent::Written { lane, len } => {
                    match lane {
                        Lane::Data => self.outbox.release_data(len),
                        Lane::Control => self.outbox.release_control(),
                    }
                    // Progress refreshes the watchdog: the transport
                    // is moving, so the stall clock restarts.
                    if self.outbox.is_empty() {
                        self.drain_stamp = None;
                    } else {
                        self.drain_stamp = Some(std::time::Instant::now());
                    }
                },
                WriterEvent::Dead { context } => {
                    self.record("transport_dead", &context);
                    self.failed = true;
                    self.writer_exited = true;
                },
                WriterEvent::Exited => {
                    self.writer_exited = true;
                },
            }
        }
        if self.outbox.is_empty() {
            self.drain_stamp = None;
        }
    }

    /// Reports a stuck writer: a full drain wait has elapsed with no
    /// completed write while bytes stay unacknowledged. The stamp is
    /// set at the first unacknowledged enqueue and refreshed by every
    /// completion, so a slow drain that keeps moving never trips it —
    /// only absolute stall for the whole wait marks the transport
    /// dead, including a single stalled frame that never fills the
    /// queue.
    pub(crate) fn drain_expired(&self) -> bool {
        match self.drain_stamp {
            Some(stamp) => {
                stamp.elapsed() >= Duration::from_millis(self.drain_wait_ms)
                    && !self.outbox.is_empty()
            },
            None => false,
        }
    }

    /// Arms the drain watchdog on a successful enqueue that leaves
    /// traffic unacknowledged.
    pub(crate) fn note_queued(&mut self) {
        if self.drain_stamp.is_none() {
            self.drain_stamp = Some(std::time::Instant::now());
        }
    }

    /// Releases the writer thread and, on a clean shutdown with bytes
    /// still unacknowledged, waits up to the negotiated drain wait
    /// for the drain so test and pipe harnesses observe complete
    /// transcripts. A failed or refused shutdown returns immediately:
    /// a dead transport will never drain, and the process exit
    /// reclaims the parked thread. A clean shutdown whose drain does
    /// not complete within the wait records the typed drain failure
    /// but keeps the session's exit code: the code reports the
    /// session outcome (EOF, terminate, fence, liveness), while the
    /// diagnostic reflects the undrained tail. A zero negotiated wait
    /// fails immediately without waiting.
    pub(crate) fn shutdown_writer(&mut self, code: i32) -> i32 {
        self.done.store(true, Ordering::Release);
        drop(self.gate_tx.take());
        // Keep the writer connected through the bounded drain: every
        // loop turn flushes deferred frames first, so completions
        // free channel room and the next flush moves more. Only after
        // the drain wait expires (or the transport proves dead) is
        // the queue disconnected and the remainder dropped with a
        // typed record. A single full try_send never decides loss.
        // A dead writer skips the wait (nothing can drain), but its
        // leftover deferred frames are still recorded below.
        let unacked = !self.writer_exited;
        let deferred =
            !self.deferred_data.is_empty() || !self.deferred_ctrl.is_empty();
        if code == EXIT_CLEAN && !self.failed && (unacked || deferred) {
            let deadline = std::time::Instant::now()
                + Duration::from_millis(self.drain_wait_ms);
            let mut death: Option<String> = None;
            loop {
                self.flush_deferred();
                if self.outbox.is_empty() {
                    break;
                }
                if self.writer_exited || self.failed {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                match self
                    .writer_events
                    .recv_timeout(Duration::from_millis(10))
                {
                    Ok(WriterEvent::Written { lane, len }) => match lane {
                        Lane::Data => self.outbox.release_data(len),
                        Lane::Control => self.outbox.release_control(),
                    },
                    Ok(WriterEvent::Dead { context }) => {
                        death = Some(context);
                        break;
                    },
                    Ok(WriterEvent::Exited) => {
                        break;
                    },
                    Err(_) => {},
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
            }
            if !self.outbox.is_empty() {
                // A dead writer explains the undrained tail with its
                // own context; only a live-but-silent drain is a
                // bare timeout.
                match death {
                    Some(context) => {
                        self.record("transport_dead", &context);
                    },
                    None => {
                        self.record(
                            "drain_timeout",
                            "shutdown drain exceeded the drain wait",
                        );
                    },
                }
            }
        }
        // Disconnecting now drops nothing silently: flushed frames
        // already went to the writer, and any deferred remainder is
        // recorded below.
        drop(self.queues.take());
        let deferred = self.deferred_data.len() + self.deferred_ctrl.len();
        if deferred > 0 {
            self.record(
                "deferred_unwritten",
                "deferred frames never reached the wire",
            );
        }
        code
    }
    pub(crate) fn terminalize_all(&mut self, reason: &str) {
        self.record("terminalize", reason);
        // Sync writer state first: a dead transport reports through
        // the event channel, and teardown must observe it before
        // deciding whether any wire reply may be attempted.
        self.drain_writer_events();
        self.prime_anchors.clear();
        // Held prime-waits resolve with the fence-terminalized shape:
        // a synthetic stale empty snapshot from the fencing
        // generation. Best-effort on a dying transport, and nothing
        // at all on a dead one: a non-draining peer is a dead
        // transport, and no wire reply is required on a dead channel.
        // Skipped writes still retire their table entries below.
        for pending in std::mem::take(&mut self.pending_looks) {
            if self.failed || self.writer_exited || self.queues.is_none() {
                self.table.retire(Direction::Inbound, pending.wire);
                continue;
            }
            let _ = self.write_value(&json!({
                "jsonrpc": "2.0",
                "id": pending.wire.to_string(),
                "result": look::fence_snapshot(
                    &pending.correlation,
                    self.binding
                        .as_ref()
                        .map(|binding| binding.adapter_generation)
                        .unwrap_or(0),
                ),
            }));
            self.table.retire(Direction::Inbound, pending.wire);
        }
        // A held turn is never completed here: an unknown outcome keeps
        // the single-flight gate until the attempt's cessation is
        // observed, and a fence hint is not cessation. Draining the
        // choice gate is safe: every open choice terminalizes now.
        self.choice_gate.drain();
        if let Some(pending) = self.pending_submit.take() {
            if pending.choice.is_some() {
                self.record("choice_cancelled", reason);
            }
            if self.turn.is_held() {
                self.record(
                    "submission_unknown",
                    "turn interrupted before terminal ack",
                );
            }
            self.table.retire(Direction::Inbound, pending.inbound_wire);
        }
        // Teardown drains persist_wires AND the PersistTracker: every
        // open persist records unknown once and both records retire,
        // so the table sweep below never double-records the same
        // write.
        for (wire, write_id) in std::mem::take(&mut self.persist_wires) {
            self.record(
                "persist_unknown",
                "open persist at teardown; no commit proof",
            );
            self.persist.retire(&write_id);
            self.table.retire(Direction::Outbound, wire);
        }
        for entry in self.table.clear() {
            if entry.kind == CallKind::Persist {
                self.record(
                    "persist_unknown",
                    "open persist at teardown; no commit proof",
                );
            }
        }
    }
}

/// Teardown accounting test: one test function by the inline-test
/// rule.
#[cfg(test)]
mod terminalize_tests {
    use std::collections::HashMap;
    use std::sync::mpsc;

    use super::super::AdapterConfig;
    use super::super::binding::CallKind;
    use super::super::handshake::{LimitsProposal, VersionOffer};
    use super::super::lifecycle::ManualClock;
    use super::super::script::parse_script;
    use super::super::writer::spawn_writer;
    use super::super::{Adapter, PendingLook};

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

    /// Teardown on a dead transport writes nothing, through either
    /// dead signal independently: with the failure flag preserved
    /// (the run loop no longer clears it before teardown), a held
    /// prime-wait still retires its table entry but enqueues no fence
    /// snapshot — and likewise when the writer is reported gone with
    /// a live in-process transport that would accept the write. The
    /// outbox stays empty and no new write is attempted in both
    /// phases; attempting the write would enqueue, re-arm failure,
    /// and fail this test.
    #[test]
    fn terminalize_writes_nothing_on_dead_transport() {
        let config = test_config();
        let clock = ManualClock::new(1_000);
        let (event_tx, event_rx) = mpsc::channel();
        let queues = spawn_writer(std::io::Cursor::new(Vec::new()), event_tx);
        let mut sink = Vec::new();
        let mut adapter =
            Adapter::new(&config, &clock, queues, event_rx, &mut sink);
        let hold_look = |adapter: &mut Adapter, wire: u64| {
            adapter
                .table
                .note_inbound(
                    wire,
                    CallKind::Look,
                    Some(format!("look-{wire}")),
                )
                .expect("inbound opens");
            adapter.pending_looks.push(PendingLook {
                wire,
                correlation: format!("look-{wire}"),
                target: "seat-1".to_string(),
                handle: format!("h-{wire}"),
                deadline_ms: 9_999_999,
                lines: None,
                offset: 0,
            });
        };
        // Phase one: failure flag preserved into teardown, writer
        // nominally live — the fence write must still be skipped.
        hold_look(&mut adapter, 10);
        adapter.failed = true;
        adapter.terminalize_all("transport_unusable");
        assert!(
            adapter.outbox.is_empty(),
            "no fence snapshot on a failed transport"
        );
        assert!(adapter.table.is_empty(), "the held look must still retire");
        // Phase two: flag clear but writer reported gone — skipped
        // again, and teardown must not re-arm failure.
        hold_look(&mut adapter, 11);
        adapter.failed = false;
        adapter.writer_exited = true;
        adapter.terminalize_all("transport_unusable");
        let outbox_empty = adapter.outbox.is_empty();
        let table_empty = adapter.table.is_empty();
        let failed = adapter.failed;
        drop(adapter);
        assert!(
            outbox_empty,
            "no fence snapshot may enqueue on a dead channel"
        );
        assert!(table_empty, "the held look must still retire");
        assert!(
            sink.iter().any(|entry| entry.code == "terminalize"),
            "teardown is still recorded: {sink:?}"
        );
        assert!(!failed, "teardown must not re-arm failure");
    }
}

/// Drain-watchdog test: one test function by the inline-test rule.
#[cfg(test)]
mod drain_tests {
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::super::Adapter;
    use super::super::AdapterConfig;
    use super::super::handshake::{LimitsProposal, VersionOffer};
    use super::super::lifecycle::ManualClock;
    use super::super::script::parse_script;
    use super::super::writer::{Lane, WriterEvent, spawn_writer};

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

    /// Completed writes refresh the stall clock: with a 1000ms drain
    /// wait and a stamp 700ms old, a partial completion (600 of 1000
    /// bytes still unacknowledged) restarts the wait — 400ms later
    /// the watchdog is quiet, and only 700ms after that does it fire.
    /// An absolute since-first-write clock would already fire at the
    /// first check and fail this test.
    #[test]
    fn completed_writes_refresh_the_stall_clock() {
        let config = test_config();
        let clock = ManualClock::new(1_000);
        let (event_tx, event_rx) = mpsc::channel();
        let completions = event_tx.clone();
        let queues = spawn_writer(std::io::Cursor::new(Vec::new()), event_tx);
        let mut sink = Vec::new();
        let mut adapter =
            Adapter::new(&config, &clock, queues, event_rx, &mut sink);
        adapter.drain_wait_ms = 1_000;
        adapter.outbox.push_data(1_000).expect("usage enqueues");
        adapter.drain_stamp = Some(
            Instant::now()
                .checked_sub(Duration::from_millis(700))
                .expect("recent instant"),
        );
        completions
            .send(WriterEvent::Written {
                lane: Lane::Data,
                len: 400,
            })
            .expect("completion delivers");
        adapter.drain_writer_events();
        assert!(
            !adapter.outbox.is_empty(),
            "600 bytes must remain unacknowledged"
        );
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            !adapter.drain_expired(),
            "steady progress must not trip the wait"
        );
        std::thread::sleep(Duration::from_millis(700));
        assert!(
            adapter.drain_expired(),
            "absolute stall past the wait must still fire"
        );
    }
}
