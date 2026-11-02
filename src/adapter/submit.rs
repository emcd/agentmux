//! `adapter.submit` execution and ack trichotomy.
//!
//! Single-flight turn gate with `RefusedBusy` as positive no-effect
//! evidence naming adapter single-flight; terminal outcomes use the
//! contract literals (`Submitted` / `TransportUnavailable` /
//! `SubmissionUnknown`) with `evidence_basis` on unknown and no-effect
//! outcomes. The gate limits turns, never Look/control progress.

use serde_json::{Value, json};

use super::Adapter;
use super::binding::{CallKind, Direction};
use super::choice::{ChoiceResolver, GateDecision};
use super::script::ScriptedChoice;
use super::{PendingChoice, PendingSubmit};

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.decode(text).ok()
}

/// Wire outcome literal for a submit terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeLiteral {
    Submitted,
    RefusedBusy,
    TransportUnavailable,
    SubmissionUnknown,
}

impl OutcomeLiteral {
    /// Returns the exact contract literal string.
    #[must_use]
    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Submitted => "Submitted",
            Self::RefusedBusy => "RefusedBusy",
            Self::TransportUnavailable => "TransportUnavailable",
            Self::SubmissionUnknown => "SubmissionUnknown",
        }
    }
}

/// Agent-effect evidence for one submit attempt, as observed by the
/// adapter's fake agent boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteEvidence {
    /// Bytes demonstrably left the outbox toward the agent.
    SubmittedOutbox,
    /// Serialization failed with demonstrably unbuffered bytes.
    SerializationFailedUnbuffered,
    /// Active-prompt refusal with demonstrably unbuffered bytes.
    PromptRefusedUnbuffered,
    /// No runtime exists to accept the write.
    NoRuntime,
    /// Write failed without proof that zero bytes left.
    FailedUnknown,
}

/// Maps attempt evidence to the contract outcome literal plus the
/// `evidence_basis` the outcome must carry.
#[must_use]
pub fn outcome_for_attempt(
    evidence: WriteEvidence,
) -> (OutcomeLiteral, &'static str) {
    match evidence {
        WriteEvidence::SubmittedOutbox => {
            (OutcomeLiteral::Submitted, "transport_ack")
        },
        WriteEvidence::SerializationFailedUnbuffered
        | WriteEvidence::PromptRefusedUnbuffered
        | WriteEvidence::NoRuntime => (
            OutcomeLiteral::TransportUnavailable,
            "provable_non_submission",
        ),
        WriteEvidence::FailedUnknown => {
            (OutcomeLiteral::SubmissionUnknown, "unproven_write")
        },
    }
}

/// Single-flight turn gate: one turn in flight per adapter. A submit
/// arriving while a turn is held is refused as `RefusedBusy` — positive
/// no-effect evidence, never relay refusal and never queued.
#[derive(Debug, Default)]
pub struct TurnGate {
    held_by: Option<String>,
}

impl TurnGate {
    /// Creates an idle gate.
    #[must_use]
    pub fn new() -> Self {
        Self { held_by: None }
    }

    /// Opens a turn for `correlation_id`, refusing while one is held.
    ///
    /// # Errors
    ///
    /// Returns the holding correlation when a turn is already in flight.
    pub fn begin(&mut self, correlation_id: String) -> Result<(), String> {
        if let Some(holder) = self.held_by.clone() {
            return Err(holder);
        }
        self.held_by = Some(correlation_id);
        Ok(())
    }

    /// Closes the held turn, returning its correlation id.
    pub fn complete(&mut self) -> Option<String> {
        self.held_by.take()
    }

    /// Reports whether a turn is currently held.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.held_by.is_some()
    }
}

impl Adapter<'_> {
    pub(crate) fn handle_submit(&mut self, envelope: &Value) -> Option<i32> {
        let Some(params) = envelope.get("params") else {
            self.answer_invalid(envelope.get("id"));
            return None;
        };
        if !self.check_binding(params) {
            self.answer_binding_mismatch(
                envelope.get("id"),
                envelope.get("params"),
            );
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
            self.answer_binding_mismatch(
                envelope.get("id"),
                envelope.get("params"),
            );
            return None;
        }
        let message_id = match self.validate_members(params) {
            Ok(message_id) => message_id,
            Err(reason) => {
                self.answer_structural(
                    envelope.get("id"),
                    &correlation,
                    &reason,
                );
                self.table.retire(Direction::Inbound, wire);
                return None;
            },
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
        let choice_correlation = format!("ch-{request_id}");
        self.table.set_correlation(
            Direction::Outbound,
            request_id,
            choice_correlation.clone(),
        );
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
                "protocol_major": self.selected_major,
                "protocol_minor": self.selected_minor,
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

    pub(crate) fn complete_submit(
        &mut self,
        id: &Value,
        inbound_wire: u64,
        correlation: &str,
        outcome: OutcomeLiteral,
    ) {
        let (literal, evidence) = match outcome {
            OutcomeLiteral::Submitted => {
                outcome_for_attempt(WriteEvidence::SubmittedOutbox)
            },
            OutcomeLiteral::TransportUnavailable => {
                outcome_for_attempt(WriteEvidence::NoRuntime)
            },
            OutcomeLiteral::SubmissionUnknown => {
                outcome_for_attempt(WriteEvidence::FailedUnknown)
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

    pub(crate) fn answer_submit(
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

    pub(crate) fn validate_members(
        &self,
        params: &Value,
    ) -> Result<String, String> {
        let members =
            params.get("members").and_then(Value::as_array).ok_or_else(
                || "submit members must be a non-empty array".to_string(),
            )?;
        if members.is_empty() {
            return Err("submit members must be a non-empty array".to_string());
        }
        let binding = self
            .binding
            .as_ref()
            .ok_or_else(|| "no captured binding for submit".to_string())?;
        for member in members {
            let target = member
                .get("target_session")
                .and_then(Value::as_str)
                .unwrap_or("");
            if target != binding.target_session {
                return Err(
                    "submit member names an unbound target".to_string()
                );
            }
            let generation =
                member.get("adapter_generation").and_then(Value::as_u64);
            if generation != Some(binding.adapter_generation) {
                return Err(
                    "submit member names a stale generation".to_string()
                );
            }
            let content_type = member
                .get("content_type")
                .and_then(Value::as_str)
                .unwrap_or("");
            if content_type.is_empty() {
                return Err("submit member needs a content type".to_string());
            }
            if !self
                .config
                .proposal
                .latency_declarations
                .contains_key(content_type)
            {
                return Err(format!("unknown content type {content_type:?}"));
            }
            let payload = member
                .get("payload_b64")
                .and_then(Value::as_str)
                .unwrap_or("");
            if base64_decode(payload).is_none() {
                return Err("submit member payload is not base64".to_string());
            }
        }
        members
            .first()
            .and_then(|member| member.get("message_id"))
            .and_then(Value::as_str)
            .filter(|message_id| !message_id.is_empty())
            .map(str::to_string)
            .ok_or_else(|| "submit member needs a message id".to_string())
    }
}
