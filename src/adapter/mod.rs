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

use binding::{
    Binding, CallKind, Direction, PendingTable, PinnedLaunch,
    establish_binding,
};
use choice::{
    ChoiceGate, ChoiceResolution, ChoiceResolver, DiagnosticEntry,
    DiagnosticSink, GateDecision,
};
use frame::{
    BOOTSTRAP_MAX_FRAME_BYTES, contract_error, parse_frame, read_frame,
    write_frame,
};
use handshake::{
    LimitsProposal, VersionOffer, hello_request, parse_hello_result,
    validate_limits, validate_selection,
};
use lifecycle::{Clock, Liveness, Outbox, ping_reply};
use look::{
    Freshness, Snapshot, SnapshotEntry, SnapshotSource, snapshot_result,
};
use persist::PersistTracker;
use script::{Script, ScriptedChoice};
use submit::{OutcomeLiteral, TurnGate, outcome_for_attempt};

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
    let mut runner = Runner::new(config, clock, writer, sink);
    runner.serve(reader)
}

struct PendingSubmit {
    correlation: String,
    inbound_wire: u64,
    choice: Option<PendingChoice>,
}

struct PendingChoice {
    wire_id: u64,
    resolver: ChoiceResolver,
}

struct Runner<'a> {
    config: &'a AdapterConfig,
    clock: &'a dyn Clock,
    writer: &'a mut dyn Write,
    sink: &'a mut dyn DiagnosticSink,
    table: PendingTable,
    binding: Option<Binding>,
    inbound_bound: usize,
    prime_anchors: HashMap<String, u64>,
    turn: TurnGate,
    choice_gate: ChoiceGate,
    persist: PersistTracker,
    outbox: Outbox,
    liveness: Option<Liveness>,
    pending_submit: Option<PendingSubmit>,
}

impl<'a> Runner<'a> {
    fn new(
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
            prime_anchors: HashMap::new(),
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

    fn serve<R: BufRead>(&mut self, reader: &mut R) -> i32 {
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

    fn handshake<R: BufRead>(&mut self, reader: &mut R) -> Result<(), ()> {
        let pinned = PinnedLaunch {
            target_session: self.config.target_session.clone(),
            adapter_profile: self.config.profile.clone(),
            adapter_executable: self.config.executable.clone(),
        };
        let request =
            hello_request(&pinned, &self.config.offer, &self.config.proposal)
                .map_err(|_| ())?;
        let hello_wire = self
            .table
            .mint(Direction::Outbound, CallKind::Handshake, None)
            .map_err(|_| ())?;
        self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": hello_wire.to_string(),
            "method": "adapter.handshakeHello",
            "params": request,
        }))
        .map_err(|_| ())?;
        let bytes = match read_frame(reader, BOOTSTRAP_MAX_FRAME_BYTES) {
            Ok(Some(bytes)) => bytes,
            _ => return Err(()),
        };
        let envelope = parse_frame(&bytes).map_err(|_| ())?;
        if envelope.get("id").and_then(Value::as_str)
            != Some(&hello_wire.to_string())
        {
            return Err(());
        }
        let result = envelope
            .get("result")
            .ok_or(())
            .and_then(|result| parse_hello_result(result).map_err(|_| ()))?;
        validate_selection(
            &self.config.offer,
            result.selected_major,
            result.selected_minor,
        )
        .map_err(|_| ())?;
        validate_limits(&self.config.proposal, &result.negotiated)
            .map_err(|_| ())?;
        self.inbound_bound = result.negotiated.max_frame_bytes as usize;
        let (binding, _) = establish_binding(
            &pinned,
            result.runtime_instance,
            result.adapter_generation,
        );
        self.table.retire(Direction::Outbound, hello_wire);
        self.binding = Some(binding);
        self.liveness = Some(Liveness::new(self.clock.now_ms(), 60_000));
        for event in &self.config.script.events.clone() {
            self.emit_lifecycle(&event.kind, &event.code);
        }
        Ok(())
    }

    fn dispatch(&mut self, bytes: &[u8]) -> Option<i32> {
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

    fn dispatch_request(
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

    fn wire_id_of(&mut self, envelope: &Value) -> Option<u64> {
        envelope.get("id")?.as_str()?.parse::<u64>().ok()
    }

    fn binding_triple_of(
        &self,
        params: &Value,
    ) -> Option<(String, String, u64)> {
        Some((
            params.get("runtime_instance")?.as_str()?.to_string(),
            params.get("target_session")?.as_str()?.to_string(),
            params.get("adapter_generation")?.as_u64()?,
        ))
    }

    fn check_binding(&self, params: &Value) -> bool {
        let Some(binding) = &self.binding else {
            return false;
        };
        let Some((runtime, target, generation)) =
            self.binding_triple_of(params)
        else {
            return false;
        };
        runtime == binding.runtime_instance
            && target == binding.target_session
            && generation == binding.adapter_generation
    }

    fn correlation_of(&self, params: &Value) -> Option<String> {
        params
            .get("correlation_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn answer_invalid(&mut self, id: Option<&Value>) {
        let _ = self.write_value(&contract_error(
            id,
            "invalid_request",
            "not a JSON-RPC 2.0 envelope",
            None,
            None,
        ));
    }

    fn answer_method_unknown(&mut self, id: Option<&Value>) {
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": id.cloned().unwrap_or(Value::Null),
            "error": { "code": -32601, "message": "method not found" },
        }));
    }

    fn answer_binding_mismatch(&mut self, id: Option<&Value>) {
        let _ = self.write_value(&contract_error(
            id,
            "binding_mismatch",
            "binding triple does not match the captured binding",
            None,
            None,
        ));
    }

    fn emit_lifecycle(&mut self, kind: &str, code: &str) {
        let Some(binding) = &self.binding else {
            return;
        };
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "method": "adapter.lifecycleEvent",
            "params": {
                "runtime_instance": binding.runtime_instance,
                "target_session": binding.target_session,
                "adapter_generation": binding.adapter_generation,
                "kind": kind,
                "code": code,
            },
        }));
    }

    fn write_value(&mut self, value: &Value) -> Result<(), ()> {
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

    fn record(&mut self, code: &str, detail: &str) {
        self.sink.record(DiagnosticEntry {
            code: code.to_string(),
            detail: detail.to_string(),
        });
    }

    fn handle_ping(&mut self, envelope: &Value) -> Option<i32> {
        let id = envelope.get("id").cloned().unwrap_or(Value::Null);
        let Some(params) = envelope.get("params") else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if !self.check_binding(params) {
            self.answer_binding_mismatch(envelope.get("id"));
            return None;
        }
        let wire = match self.wire_id_of(envelope) {
            Some(wire) => wire,
            None => {
                self.answer_invalid(envelope.get("id"));
                return None;
            },
        };
        if self.table.note_inbound(wire, CallKind::Ping, None).is_err() {
            self.answer_binding_mismatch(envelope.get("id"));
            return None;
        }
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": ping_reply(),
        }));
        self.table.retire(Direction::Inbound, wire);
        None
    }

    fn handle_look(&mut self, envelope: &Value) -> Option<i32> {
        let Some(params) = envelope.get("params") else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if !self.check_binding(params) {
            self.answer_binding_mismatch(envelope.get("id"));
            return None;
        }
        let (Some(wire), Some(correlation)) =
            (self.wire_id_of(envelope), self.correlation_of(params))
        else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if self
            .table
            .note_inbound(wire, CallKind::Look, Some(correlation.clone()))
            .is_err()
        {
            self.answer_binding_mismatch(envelope.get("id"));
            return None;
        }
        let target = params
            .get("target_session")
            .and_then(Value::as_str)
            .unwrap_or("");
        let generation = self
            .binding
            .as_ref()
            .map(|binding| binding.adapter_generation)
            .unwrap_or(0);
        let now = self.clock.now_ms();
        let snapshot = match self.config.script.snapshot_for(target) {
            Some(scripted) => {
                let first = *self
                    .prime_anchors
                    .entry(target.to_string())
                    .or_insert(now);
                if now < first.saturating_add(scripted.delay_ms) {
                    Snapshot {
                        correlation_id: correlation.clone(),
                        adapter_generation: generation,
                        entries: Vec::new(),
                        freshness: Freshness::Stale,
                        source: SnapshotSource::Synthetic,
                        stale: true,
                        discontinuity: false,
                        stale_reason_code: Some("priming".to_string()),
                    }
                } else {
                    let entries: Vec<SnapshotEntry> = scripted
                        .entries
                        .iter()
                        .map(|entry| SnapshotEntry {
                            kind: entry.kind.clone(),
                            seq: entry.seq,
                            text_base64: base64_encode(&entry.text),
                            truncated: entry.truncated,
                        })
                        .collect();
                    Snapshot {
                        correlation_id: correlation.clone(),
                        adapter_generation: generation,
                        entries,
                        freshness: Freshness::Fresh,
                        source: SnapshotSource::Live,
                        stale: false,
                        discontinuity: false,
                        stale_reason_code: None,
                    }
                }
            },
            None => Snapshot {
                correlation_id: correlation.clone(),
                adapter_generation: generation,
                entries: Vec::new(),
                freshness: Freshness::Stale,
                source: SnapshotSource::Synthetic,
                stale: true,
                discontinuity: false,
                stale_reason_code: Some("no_snapshot_scripted".to_string()),
            },
        };
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": envelope.get("id").cloned().unwrap_or(Value::Null),
            "result": snapshot_result(&snapshot),
        }));
        self.table.retire(Direction::Inbound, wire);
        None
    }

    fn handle_submit(&mut self, envelope: &Value) -> Option<i32> {
        let Some(params) = envelope.get("params") else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if !self.check_binding(params) {
            self.answer_binding_mismatch(envelope.get("id"));
            return None;
        }
        let (Some(wire), Some(correlation)) =
            (self.wire_id_of(envelope), self.correlation_of(params))
        else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if self
            .table
            .note_inbound(wire, CallKind::Submit, Some(correlation.clone()))
            .is_err()
        {
            self.answer_binding_mismatch(envelope.get("id"));
            return None;
        }
        let message_id = params
            .get("members")
            .and_then(Value::as_array)
            .and_then(|members| members.first())
            .and_then(|member| member.get("message_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let message_id = if message_id.is_empty() {
            correlation.clone()
        } else {
            message_id.to_string()
        };
        let null_id = Value::Null;
        let id = envelope.get("id").unwrap_or(&null_id);
        if self.turn.begin(correlation.clone()).is_err() {
            self.answer_submit(
                envelope.get("id"),
                &correlation,
                OutcomeLiteral::RefusedBusy,
                "adapter_single_flight",
            );
            self.table.retire(Direction::Inbound, wire);
            return None;
        }
        let turn = self.config.script.turn_for(&correlation);
        let ack = turn
            .map(|turn| turn.ack.as_outcome())
            .unwrap_or(OutcomeLiteral::Submitted);
        let choices: Vec<ScriptedChoice> =
            turn.map(|turn| turn.choices.clone()).unwrap_or_default();
        if choices.is_empty() {
            self.complete_submit(id, wire, &correlation, ack);
            return None;
        }
        let request_id = match self.table.mint(
            Direction::Outbound,
            CallKind::RequestChoice,
            None,
        ) {
            Ok(id) => id,
            Err(_) => {
                self.complete_submit(
                    id,
                    wire,
                    &correlation,
                    OutcomeLiteral::SubmissionUnknown,
                );
                return None;
            },
        };
        match self.choice_gate.request(&mut *self.sink) {
            GateDecision::Admit => {},
            GateDecision::Shed { .. } => {
                self.table.retire(Direction::Outbound, request_id);
                self.complete_submit(
                    id,
                    wire,
                    &correlation,
                    OutcomeLiteral::SubmissionUnknown,
                );
                return None;
            },
        }
        let choice_correlation = format!("ch-{request_id}");
        let choice = &choices[0];
        let deciders = if choice.deciders.is_empty() {
            vec![self.config.target_session.clone()]
        } else {
            choice.deciders.clone()
        };
        let options: Vec<Value> = choice
            .options
            .iter()
            .map(|option| {
                json!({
                    "option_id": option.option_id,
                    "name": option.name,
                    "species": option.species,
                })
            })
            .collect();
        let binding = self.binding.as_ref().expect("binding established");
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id.to_string(),
            "method": "adapter.requestChoice",
            "params": {
                "runtime_instance": binding.runtime_instance,
                "adapter_generation": binding.adapter_generation,
                "correlation_id": choice_correlation,
                "message_id": message_id,
                "target_session": binding.target_session,
                "decider_sessions": deciders,
                "title": choice.title,
                "species": choice.species,
                "details": choice.details,
                "options": options,
            },
        });
        if self.write_value(&request).is_err() {
            self.table.retire(Direction::Outbound, request_id);
            self.complete_submit(
                id,
                wire,
                &correlation,
                OutcomeLiteral::SubmissionUnknown,
            );
            return None;
        }
        self.pending_submit = Some(PendingSubmit {
            correlation,
            inbound_wire: wire,
            choice: Some(PendingChoice {
                wire_id: request_id,
                resolver: ChoiceResolver::new(),
            }),
        });
        None
    }

    fn complete_submit(
        &mut self,
        id: &Value,
        inbound_wire: u64,
        correlation: &str,
        outcome: OutcomeLiteral,
    ) {
        let (literal, evidence) = match outcome {
            OutcomeLiteral::Submitted => {
                outcome_for_attempt(submit::WriteEvidence::SubmittedOutbox)
            },
            OutcomeLiteral::TransportUnavailable => {
                outcome_for_attempt(submit::WriteEvidence::NoRuntime)
            },
            OutcomeLiteral::SubmissionUnknown => {
                outcome_for_attempt(submit::WriteEvidence::FailedUnknown)
            },
            OutcomeLiteral::RefusedBusy => {
                (OutcomeLiteral::RefusedBusy, "adapter_single_flight")
            },
        };
        let mut result = json!({
            "outcome": literal.as_wire(),
            "correlation_id": correlation,
        });
        if literal != OutcomeLiteral::Submitted {
            result["evidence_basis"] = Value::String(evidence.to_string());
        }
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }));
        self.turn.complete();
        self.table.retire(Direction::Inbound, inbound_wire);
        self.maybe_persist_turn(correlation);
    }

    fn answer_submit(
        &mut self,
        id: Option<&Value>,
        correlation: &str,
        outcome: OutcomeLiteral,
        evidence: &str,
    ) {
        let _ = self.write_value(&json!({
            "jsonrpc": "2.0",
            "id": id.cloned().unwrap_or(Value::Null),
            "result": {
                "outcome": outcome.as_wire(),
                "correlation_id": correlation,
                "evidence_basis": evidence,
            },
        }));
    }

    fn maybe_persist_turn(&mut self, correlation: &str) {
        let Some(binding) = self.binding.clone() else {
            return;
        };
        let wire =
            match self
                .table
                .mint(Direction::Outbound, CallKind::Persist, None)
            {
                Ok(wire) => wire,
                Err(_) => {
                    self.record("persist_cap", "turn marker skipped");
                    return;
                },
            };
        let tag = persist::ResumeTag {
            runtime_instance: binding.runtime_instance.clone(),
            target_session: binding.target_session.clone(),
            adapter_generation: binding.adapter_generation,
        };
        if self
            .persist
            .submit(correlation.to_string(), tag.clone())
            .is_err()
        {
            self.table.retire(Direction::Outbound, wire);
            self.record("persist_cap", "turn marker skipped");
            return;
        }
        let blob =
            base64_encode(&format!("{{\"last_turn\":\"{correlation}\"}}"));
        let request = json!({
            "jsonrpc": "2.0",
            "id": wire.to_string(),
            "method": "adapter.persistState",
            "params": {
                "runtime_instance": binding.runtime_instance,
                "target_session": binding.target_session,
                "adapter_generation": binding.adapter_generation,
                "correlation_id": format!("persist-{wire}"),
                "tag": {
                    "runtime_instance": tag.runtime_instance,
                    "target_session": tag.target_session,
                    "adapter_generation": tag.adapter_generation,
                },
                "write_id": correlation,
                "blob_b64": blob,
            },
        });
        if self.write_value(&request).is_err() {
            self.persist.retire(correlation);
            self.table.retire(Direction::Outbound, wire);
        }
    }

    fn dispatch_reply(&mut self, envelope: &Value) {
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
        match entry.kind {
            CallKind::RequestChoice => self.resolve_choice(envelope, wire),
            CallKind::Persist => {
                if envelope
                    .get("result")
                    .and_then(|result| result.get("persisted"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    self.table.retire(Direction::Outbound, wire);
                } else {
                    self.record("persist_refused", "write not committed");
                    self.table.retire(Direction::Outbound, wire);
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

    fn resolve_choice(&mut self, envelope: &Value, wire: u64) {
        let Some(mut pending) = self.pending_submit.take() else {
            self.record("orphan_choice_reply", "no pending submit");
            self.table.retire(Direction::Outbound, wire);
            return;
        };
        let Some(choice) = pending.choice.take() else {
            self.record("orphan_choice_reply", "no pending choice");
            self.table.retire(Direction::Outbound, wire);
            return;
        };
        if choice.wire_id != wire {
            self.record("mismatched_choice_reply", "discarded");
            pending.choice = Some(choice);
            self.pending_submit = Some(pending);
            return;
        }
        let mut resolver = choice.resolver;
        let mut malformed = false;
        if let Some(result) = envelope.get("result") {
            let outcome = result.get("outcome");
            let chosen = outcome.and_then(|outcome| outcome.get("chosen"));
            let cancelled =
                outcome.and_then(|outcome| outcome.get("cancelled"));
            if let Some(chosen) = chosen {
                let option = chosen.get("option_id").and_then(Value::as_str);
                let decider = chosen.get("decided_by").and_then(Value::as_str);
                match (option, decider) {
                    (Some(option_id), Some(decided_by)) => {
                        let _ = resolver.resolve(ChoiceResolution::Chosen {
                            option_id: option_id.to_string(),
                            decided_by: decided_by.to_string(),
                        });
                    },
                    _ => {
                        malformed = true;
                        let _ = resolver.cancel("malformed_choice_result");
                    },
                }
            } else if let Some(cancelled) = cancelled {
                match cancelled.get("reason_code").and_then(Value::as_str) {
                    Some(reason_code) => {
                        let decided_by = cancelled
                            .get("decided_by")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        let _ =
                            resolver.resolve(ChoiceResolution::Cancelled {
                                decided_by,
                                reason_code: reason_code.to_string(),
                            });
                    },
                    None => {
                        malformed = true;
                        let _ = resolver.cancel("malformed_choice_result");
                    },
                }
            } else {
                malformed = true;
                let _ = resolver.cancel("malformed_choice_result");
            }
        } else if let Some(error) = envelope.get("error") {
            let reason = error
                .get("data")
                .and_then(|data| data.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("relay_error")
                .to_string();
            let _ = resolver.cancel(&reason);
        } else {
            malformed = true;
            let _ = resolver.cancel("malformed_choice_reply");
        }
        self.choice_gate.release();
        let ack = if malformed {
            OutcomeLiteral::SubmissionUnknown
        } else {
            let turn = self.config.script.turn_for(&pending.correlation);
            turn.map(|turn| turn.ack.as_outcome())
                .unwrap_or(OutcomeLiteral::Submitted)
        };
        self.table.retire(Direction::Outbound, wire);
        let id = Value::String(pending.inbound_wire.to_string());
        self.complete_submit(
            &id,
            pending.inbound_wire,
            &pending.correlation,
            ack,
        );
    }

    fn handle_signal(&mut self, envelope: &Value) -> Option<i32> {
        let params = envelope.get("params")?;
        if !self.check_binding(params) {
            return None;
        }
        match params.get("signal").and_then(Value::as_str) {
            Some("terminate") => {
                self.terminalize_all("terminate");
                Some(EXIT_CLEAN)
            },
            Some("fence-hint") => {
                self.terminalize_all("fence_hint");
                None
            },
            _ => None,
        }
    }

    fn terminalize_all(&mut self, reason: &str) {
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

fn base64_encode(text: &str) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.encode(text.as_bytes())
}
