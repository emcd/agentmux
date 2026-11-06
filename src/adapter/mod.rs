//! Deterministic reference adapter for the frozen agent-adapter wire.
//!
//! Implements the ADAPTER SIDE of the contract only: newline-delimited
//! JSON-RPC 2.0 over stdio, with handshake, submit, choice, look, persist,
//! and lifecycle traffic. The module uses only std plus serde/serde_json
//! for wire and product-boundary types; fixture parsing may use the
//! existing `toml` dependency. Product-boundary imports (relay, MCP,
//! transport, acp, tmux, pty) never cross here — opaque handles only, per
//! the contract's no-back-edge requirement, enforced by
//! `scripts/lint-delivery-protocol-boundary.py`.
//!
//! Submodules follow the design's module mapping: `frame` owns
//! outbox bounds, queue caps, and the control lane; `outbound` owns
//! the lane-checked enqueue path with the drain-wait bound;
//! `teardown` owns the drain watchdog, the bounded shutdown drain,
//! and local terminalization; `writer` owns the threaded transport
//! sink with cross-lane priority scheduling; `reply` owns
//! originated-reply shape validation and admission; `choice` owns
//! originated-call in-flight caps and the pending-call table (`persist`
//! tracks its own writes); `lifecycle` owns ping/pong liveness and the
//! local diagnostic/inscription sink; per-type cancel/cleanup lives in
//! each method module, coordinated through `binding` teardown.

pub mod binding;
pub mod choice;
pub mod frame;
pub mod handshake;
pub mod lifecycle;
pub mod look;
pub mod outbound;
pub mod persist;
pub mod reply;
pub mod script;
pub mod submit;
pub mod teardown;
pub mod writer;

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use binding::{Binding, Direction, PendingTable};
use choice::{ChoiceGate, ChoiceResolver, DiagnosticEntry, DiagnosticSink};
use frame::{
    BOOTSTRAP_MAX_FRAME_BYTES, ReadEvent, ReaderShared, contract_error,
    parse_frame, read_loop,
};
use handshake::{LimitsProposal, VersionOffer};
use lifecycle::{Clock, Liveness, Outbox};
use persist::PersistTracker;
use script::Script;
use submit::TurnGate;
use writer::{FrameSink, WriterEvent, spawn_writer};

/// Configuration for one adapter run: launch inputs, the version
/// offer, limits proposal, the fixture script driving turns, and
/// adapter-local timeout bounds. The timeout bounds are adapter-local:
/// the supervisor enforces its own configured handshake and liveness
/// timeouts independently.
pub struct AdapterConfig {
    pub executable: String,
    pub profile: String,
    pub target_session: String,
    pub offer: VersionOffer,
    pub proposal: LimitsProposal,
    pub script: Script,
    pub handshake_timeout_ms: u64,
    pub liveness_timeout_ms: u64,
}

/// Clean shutdown: EOF, processed terminate, or handled liveness loss.
pub const EXIT_CLEAN: i32 = 0;
/// Binding, framing, or protocol refusal. Never raised for supervision
/// decisions, which terminalize locally instead.
pub const EXIT_REFUSAL: i32 = 2;

/// Adapter-local cap on outstanding originated persist writes. The
/// frozen hello carries no persist-specific in-flight cap, so none is
/// assumed; this bound is adapter-local.
const PERSIST_CAP: usize = 64;

/// Runs the adapter stdio loop: handshake, then one JSON-RPC frame
/// at a time until EOF, terminate, or refusal. v1 originates
/// `persistState` only for completed-turn markers (adapter-owned
/// resume state); it never fabricates agent-effect bytes.
///
/// Frame writes run on a dedicated writer thread behind bounded
/// queues: the run loop enqueues (blocking only up to the negotiated
/// drain wait) and never parks on a dead transport, so fence, cancel,
/// and terminate processing stay runnable while a write is blocked.
/// Reads run on a dedicated thread gated so the negotiated phase
/// transition is ordered before post-hello frame acquisition. On
/// return neither thread is joined: a thread parked on a silent
/// transport is released by closing the transport; the binary exits
/// the process, which reclaims both.
///
/// # Panics
///
/// Never panics on protocol input; malformed traffic is refused or
/// discarded per the contract.
pub fn run_stdio<W: Write + Send + 'static>(
    config: &AdapterConfig,
    clock: &impl Clock,
    reader: impl Read + Send + 'static,
    writer: W,
    sink: &mut impl DiagnosticSink,
) -> i32 {
    let (event_tx, event_rx) = mpsc::channel::<WriterEvent>();
    let sink_queues = spawn_writer(writer, event_tx);
    // Reject an invalid scripted configuration before any wire
    // traffic: a trusted fixture is never permission to emit a
    // non-conforming frame.
    if let Err(error) = config.script.validate_for_profile(&config.profile) {
        sink.record(DiagnosticEntry {
            code: "invalid_script_for_profile".to_string(),
            detail: error.to_string(),
        });
        return EXIT_REFUSAL;
    }
    let mut runner = Adapter::new(config, clock, sink_queues, event_rx, sink);
    runner.serve(reader)
}

/// Deterministic replay entry point: identical to [`run_stdio`] but
/// every admitted enqueue completes before the next begins. The
/// writer thread still moves the bytes (same scheduling code as
/// live), yet each enqueue waits for its own completion, so the wire
/// order is exactly the enqueue order under every thread schedule —
/// a replayed trace is a pure function of its explicit input
/// (frames, fixture, virtual-clock advances, dispatch order). Live
/// scheduling may interleave differently under contention; that is
/// outside the replay guarantee by the frozen contract's own scope
/// (replay covers identical traces, not external IO scheduling).
/// Requires a draining writer: a parked transport would stall the
/// quiesce wait, same as it stalls a live drain.
pub fn run_replay_stdio<W: Write + Send + 'static>(
    config: &AdapterConfig,
    clock: &impl Clock,
    reader: impl Read + Send + 'static,
    writer: W,
    sink: &mut impl DiagnosticSink,
) -> i32 {
    let (event_tx, event_rx) = mpsc::channel::<WriterEvent>();
    let sink_queues = spawn_writer(writer, event_tx);
    if let Err(error) = config.script.validate_for_profile(&config.profile) {
        sink.record(DiagnosticEntry {
            code: "invalid_script_for_profile".to_string(),
            detail: error.to_string(),
        });
        return EXIT_REFUSAL;
    }
    let mut runner = Adapter::new(config, clock, sink_queues, event_rx, sink);
    runner.lockstep_replay = true;
    runner.serve(reader)
}

pub(crate) enum Incoming {
    Frame(Vec<u8>),
    Eof,
    Unusable,
    /// A wait slice elapsed. `expired` is true only when the
    /// liveness deadline itself passed; otherwise a look sweep or
    /// further wait is due.
    Timeout {
        expired: bool,
    },
}

pub(crate) struct PendingSubmit {
    pub(crate) correlation: String,
    pub(crate) inbound_wire: u64,
    pub(crate) choice: Option<PendingChoice>,
}

pub(crate) struct PendingChoice {
    pub(crate) wire_id: u64,
    pub(crate) resolver: ChoiceResolver,
}

/// A look held open under prime-wait: the first poll anchored
/// availability, the snapshot is not yet available, and the
/// relay-supplied timeout has not expired. The run loop keeps
/// dispatching other traffic while the hold is pending.
pub(crate) struct PendingLook {
    pub(crate) wire: u64,
    pub(crate) correlation: String,
    pub(crate) target: String,
    pub(crate) handle: String,
    pub(crate) deadline_ms: u64,
    /// Requested window, if any: lines caps the count, offset skips
    /// that many entries from the newest end (tail-N, mirroring the
    /// relay's slicing of the same frozen params). Absent means
    /// unbounded from the start.
    pub(crate) lines: Option<u64>,
    pub(crate) offset: u64,
}

pub(crate) struct Adapter<'a> {
    pub(crate) config: &'a AdapterConfig,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) queues: Option<FrameSink>,
    pub(crate) writer_events: Receiver<WriterEvent>,
    pub(crate) writer_exited: bool,
    pub(crate) sink: &'a mut dyn DiagnosticSink,
    pub(crate) table: PendingTable,
    pub(crate) binding: Option<Binding>,
    pub(crate) selected_major: u32,
    pub(crate) selected_minor: u16,
    pub(crate) prime_anchors: HashMap<String, u64>,
    pub(crate) persist_wires: HashMap<u64, String>,
    pub(crate) pending_looks: Vec<PendingLook>,
    /// Frames admitted but not yet handed to the writer thread, per
    /// lane in exact enqueue order: a full channel defers instead of
    /// parking the run loop, and every loop turn flushes control
    /// first, then data. Every deferred byte stays counted against
    /// its Outbox reservation, so deferred memory is bounded by the
    /// negotiated caps exactly like queued bytes.
    pub(crate) deferred_data: VecDeque<Vec<u8>>,
    pub(crate) deferred_ctrl: VecDeque<Vec<u8>>,
    /// Lockstep replay: every admitted direct enqueue waits for its
    /// own completion before the run loop proceeds, so the wire order
    /// is exactly the enqueue order under every thread schedule.
    /// Set only by the deterministic replay entry point; live runs
    /// never wait inside enqueue.
    pub(crate) lockstep_replay: bool,
    /// Operative cap on concurrently held prime-waits: the proposal
    /// default until the hello result installs the negotiated value.
    /// Immediate answers retire at once and never occupy the cap.
    pub(crate) look_cap: usize,
    pub(crate) advance_idx: usize,
    pub(crate) turn: TurnGate,
    pub(crate) choice_gate: ChoiceGate,
    pub(crate) persist: PersistTracker,
    pub(crate) outbox: Outbox,
    pub(crate) liveness: Option<Liveness>,
    pub(crate) pending_submit: Option<PendingSubmit>,
    pub(crate) failed: bool,
    pub(crate) frame_bound: Arc<AtomicUsize>,
    pub(crate) read_bound: Arc<AtomicUsize>,
    pub(crate) installed: Arc<AtomicBool>,
    pub(crate) gate_tx: Option<std::sync::mpsc::Sender<()>>,
    /// Set when the run loop leaves: releases a reader parked in its
    /// rendezvous handoff, so no thread outlives the run.
    pub(crate) done: Arc<AtomicBool>,
    /// Bytes read but not yet dispatched: the reader parks instead of
    /// acquiring more once this reaches the RX queue bound, so
    /// aggregate buffered-but-undispatched bytes never exceed it.
    pub(crate) rx_queued: Arc<AtomicUsize>,
    /// Operative RX queue byte bound: the proposal default until the
    /// hello result installs the negotiated value verbatim.
    pub(crate) rx_budget: Arc<AtomicUsize>,
    pub(crate) ev_max: u32,
    pub(crate) ev_window_ms: u64,
    pub(crate) ev_window_start: Option<u64>,
    pub(crate) ev_count: u32,
    /// Operative drain wait: the proposal default until the hello
    /// result installs the negotiated value.
    pub(crate) drain_wait_ms: u64,
    /// Wall-clock instant of the last queue progress (enqueue with
    /// nothing outstanding before, or any completed write while bytes
    /// remain): a writer that completes nothing for the full drain
    /// wait marks the transport dead even when the queue never fills.
    /// Never armed in replay — the in-process writer always drains
    /// instantly.
    pub(crate) drain_stamp: Option<std::time::Instant>,
}

impl<'a> Adapter<'a> {
    pub(crate) fn new(
        config: &'a AdapterConfig,
        clock: &'a dyn Clock,
        queues: FrameSink,
        writer_events: Receiver<WriterEvent>,
        sink: &'a mut dyn DiagnosticSink,
    ) -> Self {
        Self {
            config,
            clock,
            queues: Some(queues),
            writer_events,
            writer_exited: false,
            sink,
            table: PendingTable::new(),
            binding: None,
            frame_bound: Arc::new(AtomicUsize::new(BOOTSTRAP_MAX_FRAME_BYTES)),
            read_bound: Arc::new(AtomicUsize::new(
                BOOTSTRAP_MAX_FRAME_BYTES
                    .min(config.proposal.queue_rx_bytes as usize),
            )),
            installed: Arc::new(AtomicBool::new(false)),
            gate_tx: None,
            done: Arc::new(AtomicBool::new(false)),
            rx_queued: Arc::new(AtomicUsize::new(0)),
            rx_budget: Arc::new(AtomicUsize::new(
                config.proposal.queue_rx_bytes as usize,
            )),
            selected_major: 0,
            selected_minor: 0,
            prime_anchors: HashMap::new(),
            persist_wires: HashMap::new(),
            pending_looks: Vec::new(),
            deferred_data: VecDeque::new(),
            deferred_ctrl: VecDeque::new(),
            lockstep_replay: false,
            look_cap: config.proposal.max_looks_in_flight as usize,
            advance_idx: 0,
            turn: TurnGate::new(),
            choice_gate: ChoiceGate::new(
                config.proposal.max_choices_in_flight as usize,
            ),
            persist: PersistTracker::new(PERSIST_CAP),
            outbox: Outbox::new(
                config.proposal.queue_tx_bytes as usize,
                config.proposal.max_control_pending as usize,
            ),
            liveness: None,
            pending_submit: None,
            failed: false,
            ev_max: 0,
            ev_window_ms: 0,
            ev_window_start: None,
            ev_count: 0,
            drain_wait_ms: config.proposal.drain_wait_ms,
            drain_stamp: None,
        }
    }

    pub(crate) fn serve<R: Read + Send + 'static>(
        &mut self,
        reader: R,
    ) -> i32 {
        // Depth-one handoff plus byte-budget gating bounds aggregate
        // buffered-but-undispatched bytes by the RX queue cap exactly:
        // the channel only sequences delivery while the budget gate
        // stops acquisition past the cap.
        let (tx, rx) = mpsc::sync_channel(1);
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        self.gate_tx = Some(gate_tx);
        let shared = ReaderShared {
            bound: self.read_bound.clone(),
            installed: self.installed.clone(),
            done: self.done.clone(),
            rx_queued: self.rx_queued.clone(),
            rx_budget: self.rx_budget.clone(),
        };
        thread::spawn(move || {
            read_loop(reader, tx, gate_rx, shared);
        });
        let start_ms = self.clock.now_ms();
        let code = match self.handshake(&rx, start_ms) {
            Ok(()) => self.event_loop(&rx),
            Err(()) => EXIT_REFUSAL,
        };
        self.shutdown_writer(code)
    }

    fn event_loop(&mut self, rx: &Receiver<ReadEvent>) -> i32 {
        loop {
            self.drain_writer_events();
            // Deferred frames go before new input: older enqueues
            // first, control lane before data. The run loop never
            // parks inside a send, so inbound fence, cancel,
            // terminate, and ping traffic stays serviceable while a
            // lane is full; only a full drain wait with zero
            // completed writes fails the session.
            self.flush_deferred();
            if self.failed {
                self.terminalize_all("transport_unusable");
                return EXIT_REFUSAL;
            }
            // Stuck-writer watchdog: checked at the top of every
            // loop iteration, before the liveness deadline. A dead
            // transport is the more certain local signal, so it takes
            // precedence when both fire: silence may be a quiet peer,
            // but zero completed writes past the drain wait is proof
            // the wire is gone. Arriving input never postpones this
            // independent deadline — only completed writes refresh
            // it.
            if self.drain_expired() {
                self.record(
                    "drain_timeout",
                    "blocked write exceeded the drain wait",
                );
                self.failed = true;
                self.terminalize_all("transport_unusable");
                return EXIT_REFUSAL;
            }
            let now = self.clock.now_ms();
            let live_deadline = self
                .liveness
                .as_ref()
                .map(|liveness| liveness.deadline_ms())
                .unwrap_or(u64::MAX);
            let look_deadline = self.next_look_deadline();
            // Slice long waits so held prime-waits resolve promptly on
            // a live clock without parking liveness behind them. The
            // liveness and look deadlines are distinct: a prime-wait
            // expiring is answered stale and the session continues;
            // only peer silence past the liveness deadline ends it.
            let wait_ms =
                live_deadline.min(look_deadline).saturating_sub(now).min(50);
            match self.recv(rx, now.saturating_add(wait_ms), live_deadline) {
                Incoming::Frame(bytes) => {
                    // The reader accounted these bytes against the RX
                    // queue bound on acquisition; release on dispatch.
                    self.rx_queued.fetch_sub(
                        bytes
                            .len()
                            .min(self.rx_queued.load(Ordering::Relaxed)),
                        Ordering::AcqRel,
                    );
                    self.consume_advance();
                    if let Some(code) = self.dispatch(&bytes) {
                        return code;
                    }
                    if self.failed {
                        self.terminalize_all("transport_unusable");
                        return EXIT_REFUSAL;
                    }
                    self.sweep_looks();
                    if self.failed {
                        self.terminalize_all("transport_unusable");
                        return EXIT_REFUSAL;
                    }
                },
                Incoming::Eof => {
                    self.terminalize_all("eof");
                    return EXIT_CLEAN;
                },
                Incoming::Unusable => {
                    self.terminalize_all("transport_unusable");
                    return EXIT_REFUSAL;
                },
                Incoming::Timeout { expired } => {
                    if expired {
                        self.terminalize_all("liveness_timeout");
                        return EXIT_CLEAN;
                    }
                    // A non-liveness timeout is a wait slice or a held
                    // prime-wait reaching its relay-supplied bound:
                    // answer it stale and continue the session.
                    self.sweep_looks();
                    if self.failed {
                        self.terminalize_all("transport_unusable");
                        return EXIT_REFUSAL;
                    }
                },
            }
        }
    }

    fn recv(
        &self,
        rx: &Receiver<ReadEvent>,
        wait_until_ms: u64,
        deadline_ms: u64,
    ) -> Incoming {
        // Single pass by construction: every arm returns. A wall
        // slice without input returns a non-expiry wait so the loop
        // re-evaluates on wall time even when the virtual clock is
        // frozen; virtual expiry stays decided by the deadline check
        // above, which advances move.
        let now = self.clock.now_ms();
        if now >= deadline_ms {
            return Incoming::Timeout { expired: true };
        }
        if now >= wait_until_ms {
            return Incoming::Timeout { expired: false };
        }
        let wait = Duration::from_millis(wait_until_ms.saturating_sub(now));
        match rx.recv_timeout(wait) {
            Ok(ReadEvent::Frame(bytes)) => Incoming::Frame(bytes),
            Ok(ReadEvent::Eof) => Incoming::Eof,
            Ok(ReadEvent::Unusable) => Incoming::Unusable,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Incoming::Timeout { expired: false }
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => Incoming::Unusable,
        }
    }

    /// Consumes one scheduled virtual-clock advance per post-handshake
    /// frame, so timeout expiry in replay is driven only by trace
    /// events. The live clock ignores advances.
    pub(crate) fn consume_advance(&mut self) {
        if let Some(delta) =
            self.config.script.clock_advances().get(self.advance_idx)
        {
            self.clock.advance(*delta);
            self.advance_idx = self.advance_idx.saturating_add(1);
        }
    }

    /// Earliest held prime-wait deadline, or `u64::MAX` when no look
    /// is held open.
    pub(crate) fn next_look_deadline(&self) -> u64 {
        self.pending_looks
            .iter()
            .map(|pending| pending.deadline_ms)
            .min()
            .unwrap_or(u64::MAX)
    }

    pub(crate) fn dispatch(&mut self, bytes: &[u8]) -> Option<i32> {
        let envelope = match parse_frame(bytes) {
            Ok(envelope) => envelope,
            Err(_) => {
                self.record("unparseable_frame", "discarded without reply");
                return None;
            },
        };
        if envelope.get("jsonrpc") != Some(&Value::String("2.0".to_string())) {
            self.answer_invalid(envelope.get("id"));
            return None;
        }
        // Liveness resets on valid frames only. Requests observe at
        // admission inside each handler — only a fully validated
        // request (binding match plus required params) proves the
        // peer alive; malformed requests and bare version markers
        // leave the deadline unchanged. Replies observe on
        // shape-validity (see dispatch_reply).
        if let Some(method) = envelope.get("method").and_then(Value::as_str) {
            return self.dispatch_request(method, &envelope);
        }
        if self.dispatch_reply(&envelope) {
            let now = self.clock.now_ms();
            if let Some(liveness) = &mut self.liveness {
                liveness.observe(now);
            }
        }
        None
    }

    pub(crate) fn dispatch_request(
        &mut self,
        method: &str,
        envelope: &Value,
    ) -> Option<i32> {
        match method {
            "adapter.submit" => self.handle_submit(envelope),
            "relay.look" => self.handle_look(envelope),
            "relay.ping" => self.handle_ping(envelope),
            "relay.supervisionSignal" => self.handle_signal(envelope),
            _ => {
                // An unknown method proves a live peer only when its
                // params carry the binding this connection captured:
                // field types alone are not enough, and a foreign or
                // missing binding never resets liveness.
                let bound = envelope
                    .get("params")
                    .is_some_and(|params| self.check_binding(params));
                if bound {
                    self.observe_liveness();
                }
                self.answer_method_unknown(envelope.get("id"));
                None
            },
        }
    }

    /// Records one valid-frame arrival for liveness. Called only at
    /// admission points — never on validation failures, bare markers,
    /// or shape-invalid replies.
    pub(crate) fn observe_liveness(&mut self) {
        let now = self.clock.now_ms();
        if let Some(liveness) = &mut self.liveness {
            liveness.observe(now);
        }
    }

    pub(crate) fn wire_id_of(&mut self, envelope: &Value) -> Option<u64> {
        envelope.get("id")?.as_str()?.parse::<u64>().ok()
    }

    pub(crate) fn binding_triple_of(
        &self,
        params: &Value,
    ) -> Option<(String, String, u64)> {
        Some((
            params.get("runtime_instance")?.as_str()?.to_string(),
            params.get("target_session")?.as_str()?.to_string(),
            params.get("adapter_generation")?.as_u64()?,
        ))
    }

    pub(crate) fn check_binding(&self, params: &Value) -> bool {
        let Some(binding) = &self.binding else {
            return false;
        };
        let Some((runtime, target, generation)) =
            self.binding_triple_of(params)
        else {
            return false;
        };
        if runtime != binding.runtime_instance
            || target != binding.target_session
            || generation != binding.adapter_generation
        {
            return false;
        }
        let major = params.get("protocol_major").and_then(Value::as_u64);
        let minor = params.get("protocol_minor").and_then(Value::as_u64);
        major == Some(u64::from(self.selected_major))
            && minor == Some(u64::from(self.selected_minor))
    }

    pub(crate) fn correlation_of(&self, params: &Value) -> Option<String> {
        params
            .get("correlation_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub(crate) fn answer_invalid(&mut self, id: Option<&Value>) {
        let _ = self.write_value(&contract_error(
            id,
            "invalid_request",
            "not a JSON-RPC 2.0 envelope",
            None,
            None,
        ));
    }

    pub(crate) fn answer_method_unknown(&mut self, id: Option<&Value>) {
        let Some(id) = id else {
            // Notifications carry no id and receive no reply — not
            // even an error. A JSON-RPC notification is never
            // answered; the diagnostic sink records the discard while
            // the wire stays silent. Liveness was already observed by
            // the caller when the params carried the binding.
            self.record(
                "unknown_method_notification",
                "notification discarded without reply",
            );
            return;
        };
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": id.clone(),
            "error": { "code": -32601, "message": "method not found" },
        }));
    }

    pub(crate) fn answer_binding_mismatch(
        &mut self,
        id: Option<&Value>,
        params: Option<&Value>,
    ) {
        let correlation = params
            .and_then(|params| params.get("correlation_id"))
            .and_then(Value::as_str);
        let _ = self.write_value(&contract_error(
            id,
            "binding_mismatch",
            "binding triple or protocol version does not match",
            None,
            correlation,
        ));
    }

    pub(crate) fn answer_structural(
        &mut self,
        id: Option<&Value>,
        correlation: &str,
        reason: &str,
    ) {
        let _ = self.write_value(&contract_error(
            id,
            "validation_structural",
            reason,
            None,
            Some(correlation),
        ));
    }

    /// Emits a lifecycle event on the control lane, enforcing the
    /// negotiated event rate: coalescible kinds drop with a diagnostic
    /// past the cap, authoritative kinds terminalize instead — never
    /// silently dropped, never queued unboundedly. Returns an exit code
    /// when the session must end.
    pub(crate) fn emit_lifecycle(
        &mut self,
        kind: &str,
        code: &str,
    ) -> Option<i32> {
        let coalescible = matches!(kind, "progress" | "heartbeat");
        let now = self.clock.now_ms();
        let window_expired = self.ev_window_start.is_none_or(|start| {
            now.saturating_sub(start) >= self.ev_window_ms
        });
        if window_expired {
            self.ev_window_start = Some(now);
            self.ev_count = 0;
        }
        if self.ev_count >= self.ev_max {
            if coalescible {
                self.record("event_coalesced", kind);
                return None;
            }
            self.terminalize_all("event_flood");
            return Some(EXIT_REFUSAL);
        }
        self.ev_count = self.ev_count.saturating_add(1);
        let Some(binding) = &self.binding else {
            return None;
        };
        let result = self.write_control(&json!({
            "jsonrpc": "2.0",
            "method": "adapter.lifecycleEvent",
            "params": {
                "protocol_major": self.selected_major,
                "protocol_minor": self.selected_minor,
                "runtime_instance": binding.runtime_instance,
                "target_session": binding.target_session,
                "adapter_generation": binding.adapter_generation,
                "kind": kind,
                "code": code,
            },
        }));
        if result.is_err() {
            self.terminalize_all("lifecycle_write_failed");
            return Some(EXIT_REFUSAL);
        }
        None
    }

    pub(crate) fn record(&mut self, code: &str, detail: &str) {
        self.sink.record(DiagnosticEntry {
            code: code.to_string(),
            detail: detail.to_string(),
        });
    }

    /// Sweeps held prime-waits: answers snapshots available since the
    /// anchor fresh, and expires overdue waits stale-tagged priming.
    /// Unready waits stay held while other traffic proceeds.
    pub(crate) fn sweep_looks(&mut self) {
        let now = self.clock.now_ms();
        let mut pending = std::mem::take(&mut self.pending_looks);
        let mut held = Vec::with_capacity(pending.len());
        for look in pending.drain(..) {
            let anchor =
                self.prime_anchors.get(&look.handle).copied().unwrap_or(now);
            let available = self.snapshot_available(&look.target, anchor, now);
            if available {
                self.answer_fresh_look(&look);
                self.table.retire(Direction::Inbound, look.wire);
            } else if now >= look.deadline_ms {
                self.answer_priming_look(&look);
                self.table.retire(Direction::Inbound, look.wire);
            } else {
                held.push(look);
            }
        }
        self.pending_looks = held;
    }

    /// Reports whether the scripted snapshot for `target` is available
    /// at `now` given its first-poll `anchor`: immediately when no
    /// delay is scripted, or once anchor plus delay has passed.
    /// Unscripted targets never become available.
    pub(crate) fn snapshot_available(
        &self,
        target: &str,
        anchor: u64,
        now: u64,
    ) -> bool {
        match self.config.script.snapshot_for(target) {
            None => false,
            Some(scripted) => now >= anchor.saturating_add(scripted.delay_ms),
        }
    }
}
