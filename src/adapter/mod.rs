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
//! Submodules follow the design's nine-module mapping: `frame` owns
//! outbox bounds, queue caps, and the control lane; `choice` owns
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
pub mod persist;
pub mod script;
pub mod submit;

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use binding::{Binding, CallKind, Direction, PendingTable};
use choice::{ChoiceGate, ChoiceResolver, DiagnosticEntry, DiagnosticSink};
use frame::{
    BOOTSTRAP_MAX_FRAME_BYTES, contract_error, parse_frame, read_frame,
    write_frame,
};
use handshake::{LimitsProposal, VersionOffer};
use lifecycle::{Clock, Liveness, Outbox};
use persist::PersistTracker;
use script::Script;
use submit::TurnGate;

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
/// Reads run on a dedicated thread so liveness and handshake
/// deadlines fire while the peer is silent; the main loop admits one
/// frame at a time under the negotiated bound. On return the reader
/// thread may stay blocked on a silent transport: close the transport
/// to release it. The binary exits the process, which reclaims it.
///
/// # Panics
///
/// Never panics on protocol input; malformed traffic is refused or
/// discarded per the contract.
pub fn run_stdio<R: BufRead + Send + 'static>(
    config: &AdapterConfig,
    clock: &impl Clock,
    reader: R,
    writer: &mut impl Write,
    sink: &mut impl DiagnosticSink,
) -> i32 {
    let mut runner = Adapter::new(config, clock, writer, sink);
    runner.serve(reader)
}

#[derive(Debug)]
pub(crate) enum ReadEvent {
    Frame(Vec<u8>),
    Eof,
    Unusable,
}

fn read_loop(
    reader: impl BufRead + Send,
    bound: Arc<AtomicUsize>,
    tx: SyncSender<ReadEvent>,
) {
    let mut reader = reader;
    loop {
        let limit = bound.load(Ordering::Relaxed);
        match read_frame(&mut reader, limit) {
            Ok(Some(bytes)) => {
                if tx.send(ReadEvent::Frame(bytes)).is_err() {
                    return;
                }
            },
            Ok(None) => {
                let _ = tx.send(ReadEvent::Eof);
                return;
            },
            Err(_) => {
                let _ = tx.send(ReadEvent::Unusable);
                return;
            },
        }
    }
}

pub(crate) enum Incoming {
    Frame(Vec<u8>),
    Eof,
    Unusable,
    Timeout,
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

pub(crate) struct Adapter<'a> {
    pub(crate) config: &'a AdapterConfig,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) writer: &'a mut dyn Write,
    pub(crate) sink: &'a mut dyn DiagnosticSink,
    pub(crate) table: PendingTable,
    pub(crate) binding: Option<Binding>,
    pub(crate) selected_major: u32,
    pub(crate) selected_minor: u16,
    pub(crate) prime_anchors: HashMap<String, u64>,
    pub(crate) persist_wires: HashMap<u64, String>,
    pub(crate) turn: TurnGate,
    pub(crate) choice_gate: ChoiceGate,
    pub(crate) persist: PersistTracker,
    pub(crate) outbox: Outbox,
    pub(crate) liveness: Option<Liveness>,
    pub(crate) pending_submit: Option<PendingSubmit>,
    pub(crate) failed: bool,
    pub(crate) frame_bound: Arc<AtomicUsize>,
    pub(crate) ev_max: u32,
    pub(crate) ev_window_ms: u64,
    pub(crate) ev_window_start: Option<u64>,
    pub(crate) ev_count: u32,
}

impl<'a> Adapter<'a> {
    pub(crate) fn new(
        config: &'a AdapterConfig,
        clock: &'a dyn Clock,
        writer: &'a mut dyn Write,
        sink: &'a mut dyn DiagnosticSink,
    ) -> Self {
        Self {
            config,
            clock,
            writer,
            sink,
            table: PendingTable::new(),
            binding: None,
            frame_bound: Arc::new(AtomicUsize::new(BOOTSTRAP_MAX_FRAME_BYTES)),
            selected_major: 0,
            selected_minor: 0,
            prime_anchors: HashMap::new(),
            persist_wires: HashMap::new(),
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
        }
    }

    pub(crate) fn serve<R: BufRead + Send + 'static>(
        &mut self,
        reader: R,
    ) -> i32 {
        let (tx, rx) = mpsc::sync_channel(16);
        let frame_bound = self.frame_bound.clone();
        thread::spawn(move || read_loop(reader, frame_bound, tx));
        let start_ms = self.clock.now_ms();
        match self.handshake(&rx, start_ms) {
            Ok(()) => {},
            Err(()) => return EXIT_REFUSAL,
        }
        loop {
            let deadline = self
                .liveness
                .as_ref()
                .map(|liveness| liveness.deadline_ms())
                .unwrap_or(u64::MAX);
            match self.recv(&rx, deadline) {
                Incoming::Frame(bytes) => {
                    if let Some(code) = self.dispatch(&bytes) {
                        return code;
                    }
                    if std::mem::take(&mut self.failed) {
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
                Incoming::Timeout => {
                    self.terminalize_all("liveness_timeout");
                    return EXIT_CLEAN;
                },
            }
        }
    }

    fn recv(&self, rx: &Receiver<ReadEvent>, deadline_ms: u64) -> Incoming {
        loop {
            let now = self.clock.now_ms();
            if now >= deadline_ms {
                return Incoming::Timeout;
            }
            let wait = Duration::from_millis(deadline_ms.saturating_sub(now));
            match rx.recv_timeout(wait) {
                Ok(ReadEvent::Frame(bytes)) => {
                    return Incoming::Frame(bytes);
                },
                Ok(ReadEvent::Eof) => return Incoming::Eof,
                Ok(ReadEvent::Unusable) => return Incoming::Unusable,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Incoming::Unusable;
                },
            }
        }
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
        // Liveness resets on valid frames only: parseable envelopes
        // with a JSON-RPC version marker, whether or not the binding
        // or admission checks that follow succeed.
        let now = self.clock.now_ms();
        if let Some(liveness) = &mut self.liveness {
            liveness.observe(now);
        }
        if let Some(method) = envelope.get("method").and_then(Value::as_str) {
            return self.dispatch_request(method, &envelope);
        }
        self.dispatch_reply(&envelope);
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
                self.answer_method_unknown(envelope.get("id"));
                None
            },
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
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": id.cloned().unwrap_or(Value::Null),
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

    pub(crate) fn write_value(&mut self, value: &Value) -> Result<(), ()> {
        let bytes = serde_json::to_vec(value).map_err(|_| ())?;
        if self.outbox.push_data(bytes.len()).is_err() {
            self.record("outbox_full", "response dropped at queue cap");
            self.failed = true;
            return Err(());
        }
        // Operative bound is the negotiated maximum; the bootstrap
        // ceiling applies while frame_bound still holds it.
        let bound = self.frame_bound.load(Ordering::Relaxed);
        let result = write_frame(&mut *self.writer, &bytes, bound);
        self.outbox.release_data(bytes.len());
        if result.is_err() {
            self.record("transport_write_failed", "terminalizing");
            self.failed = true;
        }
        result.map_err(|_| ())
    }

    /// Writes a control-lane frame (lifecycle events, ping replies):
    /// exempt from data caps within its own negotiated control bound.
    /// Admission failure escalates: the session terminalizes rather
    /// than dropping or silently queueing control traffic.
    pub(crate) fn write_control(&mut self, value: &Value) -> Result<(), ()> {
        let bytes = serde_json::to_vec(value).map_err(|_| ())?;
        if self.outbox.push_control().is_err() {
            self.record(
                "control_lane_full",
                "control admission escalates to terminalization",
            );
            self.failed = true;
            return Err(());
        }
        let bound = self.frame_bound.load(Ordering::Relaxed);
        let result = write_frame(&mut *self.writer, &bytes, bound);
        self.outbox.release_control();
        if result.is_err() {
            self.record("control_write_failed", "terminalizing");
            self.failed = true;
        }
        result.map_err(|_| ())
    }

    pub(crate) fn record(&mut self, code: &str, detail: &str) {
        self.sink.record(DiagnosticEntry {
            code: code.to_string(),
            detail: detail.to_string(),
        });
    }

    pub(crate) fn dispatch_reply(&mut self, envelope: &Value) {
        let id = match envelope.get("id").and_then(Value::as_str) {
            Some(id) => id,
            None => {
                self.record("reply_without_id", "discarded without admission");
                return;
            },
        };
        let Ok(wire) = id.parse::<u64>() else {
            self.record("reply_with_bad_id", "discarded without admission");
            return;
        };
        let entry = match self.table.admit(Direction::Outbound, wire) {
            Some(entry) => entry.clone(),
            None => {
                self.record(
                    "unknown_reply",
                    "discarded without wire error in reply",
                );
                return;
            },
        };
        let has_result = envelope.get("result").is_some();
        let has_error = envelope.get("error").is_some();
        if has_result == has_error {
            self.record(
                "result_error_exclusive",
                "reply carries both or neither; discarded",
            );
            return;
        }
        let code = envelope
            .get("error")
            .and_then(|error| error.get("data"))
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str);
        let exempt = code == Some("correlation_invalid");
        if !exempt {
            let echoed = envelope
                .get("result")
                .and_then(|result| result.get("correlation_id"))
                .and_then(Value::as_str)
                .or_else(|| {
                    envelope
                        .get("error")
                        .and_then(|error| error.get("data"))
                        .and_then(|data| data.get("correlation_id"))
                        .and_then(Value::as_str)
                });
            match (&entry.correlation_id, echoed) {
                (Some(expected), Some(actual)) if expected == actual => {},
                (None, _) => {},
                _ => {
                    self.record(
                        "correlation_echo_mismatch",
                        "reply kept open pending a valid echo",
                    );
                    return;
                },
            }
        }
        match entry.kind {
            CallKind::RequestChoice => self.resolve_choice(envelope, wire),
            CallKind::Persist => {
                let persisted = envelope
                    .get("result")
                    .and_then(|result| result.get("persisted"))
                    .and_then(Value::as_bool);
                let refused = envelope
                    .get("result")
                    .and_then(|result| result.get("refusal_code"))
                    .and_then(Value::as_str)
                    .is_some();
                match (persisted, refused, code) {
                    (Some(true), false, _) => {
                        if let Some(write_id) =
                            self.persist_wires.remove(&wire)
                        {
                            self.persist.retire(&write_id);
                        }
                        self.table.retire(Direction::Outbound, wire);
                    },
                    (Some(false), true, _) => {
                        self.record("persist_refused", "write not committed");
                        if let Some(write_id) =
                            self.persist_wires.remove(&wire)
                        {
                            self.persist.retire(&write_id);
                        }
                        self.table.retire(Direction::Outbound, wire);
                    },
                    (_, _, Some("correlation_invalid")) => {
                        self.record(
                            "persist_correlation_invalid",
                            "write dead on arrival",
                        );
                        if let Some(write_id) =
                            self.persist_wires.remove(&wire)
                        {
                            self.persist.retire(&write_id);
                        }
                        self.table.retire(Direction::Outbound, wire);
                    },
                    _ => {
                        self.record(
                            "persist_outcome_malformed",
                            "uncertain write kept open",
                        );
                        self.table.retire(Direction::Outbound, wire);
                    },
                }
                let _ = entry;
            },
            CallKind::Handshake
            | CallKind::Submit
            | CallKind::Look
            | CallKind::Ping => {
                self.record(
                    "unexpected_reply_direction",
                    "discarded without admission",
                );
                self.table.retire(Direction::Outbound, wire);
            },
        }
    }

    pub(crate) fn terminalize_all(&mut self, reason: &str) {
        self.record("terminalize", reason);
        self.prime_anchors.clear();
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
