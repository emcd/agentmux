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
/// offer, limits proposal, and the fixture script driving turns.
pub struct AdapterConfig {
    pub executable: String,
    pub profile: String,
    pub target_session: String,
    pub offer: VersionOffer,
    pub proposal: LimitsProposal,
    pub script: Script,
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

/// Runs the adapter stdio loop: handshake, then one JSON-RPC frame at
/// a time until EOF, terminate, or refusal. v1 originates
/// `persistState` only for completed-turn markers (adapter-owned
/// resume state); it never fabricates agent-effect bytes.
///
/// # Panics
///
/// Never panics on protocol input; malformed traffic is refused or
/// discarded per the contract.
pub fn run_stdio(
    config: &AdapterConfig,
    clock: &impl Clock,
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    sink: &mut impl DiagnosticSink,
) -> i32 {
    let mut runner = Adapter::new(config, clock, writer, sink);
    runner.serve(reader)
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
    pub(crate) inbound_bound: usize,
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
            inbound_bound: BOOTSTRAP_MAX_FRAME_BYTES,
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
        }
    }

    pub(crate) fn serve<R: BufRead>(&mut self, reader: &mut R) -> i32 {
        if self.handshake(reader).is_err() {
            return EXIT_REFUSAL;
        }
        loop {
            let now = self.clock.now_ms();
            if let Some(liveness) = &self.liveness
                && liveness.is_expired(now)
            {
                self.terminalize_all("liveness_timeout");
                return EXIT_CLEAN;
            }
            let frame = match read_frame(reader, self.inbound_bound) {
                Ok(frame) => frame,
                Err(_) => {
                    self.terminalize_all("transport_unusable");
                    return EXIT_REFUSAL;
                },
            };
            let Some(bytes) = frame else {
                self.terminalize_all("eof");
                return EXIT_CLEAN;
            };
            if let Some(liveness) = &mut self.liveness {
                liveness.observe(now);
            }
            if let Some(code) = self.dispatch(&bytes) {
                return code;
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

    pub(crate) fn write_value(&mut self, value: &Value) -> Result<(), ()> {
        let bytes = serde_json::to_vec(value).map_err(|_| ())?;
        if self.outbox.push_data(bytes.len()).is_err() {
            self.record("outbox_full", "response dropped at queue cap");
            return Err(());
        }
        // Operative bound is the negotiated maximum; the bootstrap
        // ceiling applies only before the handshake completes (when
        // inbound_bound still holds it).
        let result =
            write_frame(&mut *self.writer, &bytes, self.inbound_bound);
        self.outbox.release_data(bytes.len());
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
        if let Some(pending) = self.pending_submit.take() {
            if pending.choice.is_some() {
                self.record("choice_cancelled", reason);
            }
            if self.turn.complete().is_some() {
                self.record(
                    "submission_unknown",
                    "turn interrupted before terminal ack",
                );
            }
            self.table.retire(Direction::Inbound, pending.inbound_wire);
        } else {
            self.turn.complete();
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
