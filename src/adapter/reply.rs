//! Replies to originated calls: shape-validated admission with keep-open semantics.
//!
//! Shape validation is specific to the pending method and precedes
//! echo checks: a shape-invalid reply — including a well-formed
//! result for the WRONG call — resolves nothing and the entry stays
//! open for a later valid terminal. A valid reply is admitted by
//! pending-table match with the contract's echo exemptions (standard
//! transport errors and correlation_invalid carry no echo requirement);
//! results always echo. An admitted reply with a mismatched echo is
//! kept open, never resolved. Persist terminals retire the call and
//! its tracker entry only on a valid outcome; an uncertain write is
//! never recorded as persisted without commit proof.

use serde_json::Value;

use super::Adapter;
use super::binding::{CallKind, Direction};

impl Adapter<'_> {
    /// Routes one reply to an originated call. Returns true iff the
    /// reply is shape-valid (driving liveness). Error shape is
    /// kind-independent and checked first, so a malformed error never
    /// counts as liveness even for an unknown id. A present echo must
    /// be a string even when its absence would be exempt. Results are
    /// validated against the pending method's shape; a shape-invalid
    /// reply resolves nothing — the entry stays open for a later
    /// valid terminal. Admission is by pending-table match with the
    /// echo exemptions the contract grants (standard transport errors
    /// and correlation_invalid carry no echo requirement). Results
    /// always echo their correlation: a missing echo is malformed
    /// and resets nothing, though the call still stays open.
    pub(crate) fn dispatch_reply(&mut self, envelope: &Value) -> bool {
        let id = match envelope.get("id").and_then(Value::as_str) {
            Some(id) => id,
            None => {
                self.record("reply_without_id", "discarded without admission");
                return false;
            },
        };
        let Ok(wire) = id.parse::<u64>() else {
            self.record("reply_with_bad_id", "discarded without admission");
            return false;
        };
        let has_result = envelope.get("result").is_some();
        let has_error = envelope.get("error").is_some();
        if has_result == has_error {
            self.record(
                "result_error_exclusive",
                "reply carries both or neither; discarded",
            );
            return false;
        }
        // Error shape is kind-independent: a malformed error resolves
        // nothing and resets nothing, whether or not its id names an
        // open entry.
        if let Some(error) = envelope.get("error")
            && !Self::error_shape_valid(error)
        {
            self.record(
                "malformed_reply_error",
                "reply error fails its shape; kept open",
            );
            return false;
        }
        // Echo exemptions first, and only for errors: JSON-RPC
        // standard transport/parse errors name no correlation by
        // construction, and correlation_invalid rejects the
        // correlation it would echo. Any other numeric code —
        // including non-standard codes like 42 and contract errors
        // other than correlation_invalid — still requires its echo.
        // Everything else requires its echo — including replies to
        // unknown ids, which are discarded but still prove a live
        // peer only when well-formed.
        let error_code = envelope
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_i64);
        let contract_code = envelope
            .get("error")
            .and_then(|error| error.get("data"))
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str);
        let exempt = envelope.get("error").is_some()
            && (error_code.is_some_and(Self::standard_error_code)
                || (error_code == Some(-32000)
                    && contract_code == Some("correlation_invalid")));
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
        // A present echo must be a string even when its absence
        // would be exempt: an exemption waives the echo requirement,
        // never the type of a field the peer actually sent.
        let raw_echo = envelope
            .get("result")
            .and_then(|result| result.get("correlation_id"))
            .or_else(|| {
                envelope
                    .get("error")
                    .and_then(|error| error.get("data"))
                    .and_then(|data| data.get("correlation_id"))
            });
        if let Some(value) = raw_echo
            && value.as_str().is_none()
        {
            self.record(
                if has_result {
                    "malformed_reply_result"
                } else {
                    "malformed_reply_error"
                },
                "reply correlation must be a string when present",
            );
            return false;
        }
        if !exempt && echoed.is_none() {
            // A missing required echo is malformed and resets
            // nothing, whether or not its id names an open entry.
            self.record(
                if has_result {
                    "malformed_reply_result"
                } else {
                    "malformed_reply_error"
                },
                "reply missing its required echo; kept open",
            );
            return false;
        }
        let entry = match self.table.admit(Direction::Outbound, wire) {
            Some(entry) => entry.clone(),
            None => {
                // Unknown id with a valid shape and its required
                // echo: discarded without a wire error, but the
                // well-formed bytes still prove a live peer.
                // Malformed results never reach here — the generic
                // check below runs before admission.
                if let Some(result) = envelope.get("result")
                    && !Self::generic_result_shape_valid(result)
                {
                    self.record(
                        "malformed_reply_result",
                        "reply result fails its shape; kept open",
                    );
                    return false;
                }
                self.record(
                    "unknown_reply",
                    "discarded without wire error in reply",
                );
                return true;
            },
        };
        // Shape validation is specific to the pending method: a
        // well-formed result for the WRONG call is still a framing
        // failure — a persist-shaped result resolves no choice and a
        // choice-shaped result resolves no persist. Anything invalid
        // here resolves nothing; the entry stays open. Errors were
        // validated above, so only results reach the shape gate here.
        let shape_ok = match entry.kind {
            CallKind::RequestChoice => {
                envelope.get("result").is_none_or(|result| {
                    Self::choice_outcome_valid(
                        result.get("outcome").unwrap_or(&Value::Null),
                    ) && result.get("persisted").is_none()
                })
            },
            CallKind::Persist => envelope
                .get("result")
                .is_none_or(Self::persist_result_valid),
            CallKind::Handshake
            | CallKind::Submit
            | CallKind::Look
            | CallKind::Ping => {
                self.record(
                    "unexpected_reply_direction",
                    "discarded without admission",
                );
                self.table.retire(Direction::Outbound, wire);
                return true;
            },
        };
        if !shape_ok {
            // Kind-specific diagnostics: the persist tracker owns its
            // writes, so a malformed persist outcome on its own call
            // is reported on its own code; every other shape failure
            // — including a well-formed result for the wrong call —
            // is a generic reply-shape failure. All keep the call open.
            let persist_shaped = envelope
                .get("result")
                .is_some_and(|result| result.get("persisted").is_some());
            if entry.kind == CallKind::Persist && persist_shaped {
                self.record(
                    "persist_outcome_malformed",
                    "uncertain write kept open",
                );
            } else {
                self.record(
                    "malformed_reply_result",
                    "reply result fails its shape; kept open",
                );
            }
            return false;
        }
        // Echo match for an admitted, non-exempt reply: a present
        // echo that names the open entry admits; any other present
        // echo keeps the call open. Missing echoes never reach here
        // (refused above), and exempt errors skip this match entirely:
        // an exemption removes the echo requirement rather than
        // adding an echo-matching criterion, so exempt errors admit
        // by table match with no echo, a matching echo, or a
        // non-matching echo alike.
        if !exempt {
            match (&entry.correlation_id, echoed) {
                (Some(expected), Some(actual)) if expected == actual => {},
                (None, _) => {},
                _ => {
                    self.record(
                        "correlation_echo_mismatch",
                        "reply kept open pending a valid echo",
                    );
                    return true;
                },
            }
        }
        match entry.kind {
            CallKind::RequestChoice => self.resolve_choice(envelope, wire),
            CallKind::Persist => self.resolve_persist(envelope, wire),
            CallKind::Handshake
            | CallKind::Submit
            | CallKind::Look
            | CallKind::Ping => {
                self.record(
                    "unexpected_reply_direction",
                    "discarded as unreachable",
                );
                self.table.retire(Direction::Outbound, wire);
            },
        }
        true
    }

    /// Validates a reply `result` generically (no pending call to
    /// check it against): exactly one terminal kind — a well-formed
    /// choice outcome or a well-formed persist outcome, never both,
    /// never neither. Used for unknown-id replies, where admission
    /// cannot select a method-specific validator.
    fn generic_result_shape_valid(result: &Value) -> bool {
        let choice = result.get("outcome");
        let persisted = result.get("persisted");
        match (choice, persisted) {
            (Some(outcome), None) => Self::choice_outcome_valid(outcome),
            (None, Some(_)) => Self::persist_result_valid(result),
            _ => false,
        }
    }

    /// Validates a choice `outcome`: an externally-tagged object with
    /// exactly one key — chosen with required option/decider strings,
    /// or cancelled with a required namespaced reason code and
    /// optional decider/reason text. Both keys, neither key, an extra
    /// key on the OUTER discriminator, or mistyped required/optional
    /// fields are all typed framing failures. Unknown minor fields
    /// INSIDE either payload are permitted forward-compat content:
    /// the exactly-one rule constrains the discriminator, not the
    /// payloads.
    fn choice_outcome_valid(outcome: &Value) -> bool {
        let Some(object) = outcome.as_object() else {
            return false;
        };
        if object.len() != 1 {
            return false;
        }
        if let Some(chosen) = object.get("chosen") {
            let Some(fields) = chosen.as_object() else {
                return false;
            };
            return matches!(
                (
                    fields.get("option_id").and_then(Value::as_str),
                    fields.get("decided_by").and_then(Value::as_str),
                ),
                (Some(_), Some(_))
            );
        }
        if let Some(cancelled) = object.get("cancelled") {
            // Optional decider and reason text are type-checked when
            // present; unknown minor fields are permitted
            // forward-compat content like in `chosen`. The outer
            // outcome discriminator still requires exactly one key.
            let Some(fields) = cancelled.as_object() else {
                return false;
            };
            let reason = fields.get("reason_code").and_then(Value::as_str);
            let decider = fields
                .get("decided_by")
                .is_none_or(|value| value.as_str().is_some());
            let reason_text = fields
                .get("reason")
                .is_none_or(|value| value.as_str().is_some());
            return reason.is_some() && decider && reason_text;
        }
        false
    }

    /// Reports whether a numeric error code is a JSON-RPC standard
    /// transport/parse error, which names no correlation by
    /// construction and carries no echo requirement. Contract errors
    /// (-32000) and any other numeric code still require their echo
    /// unless they are correlation_invalid rejections.
    fn standard_error_code(code: i64) -> bool {
        matches!(code, -32700 | -32600 | -32601 | -32602 | -32603)
    }

    /// Validates a persist `result`: `persisted` is a required bool;
    /// true prohibits any refusal code (key presence, not type), and
    /// false requires a refusal code in the contract set.
    fn persist_result_valid(result: &Value) -> bool {
        let persisted = result.get("persisted").and_then(Value::as_bool);
        let refusal = result.get("refusal_code");
        match (persisted, refusal) {
            (Some(true), None) => true,
            (Some(false), Some(Value::String(code))) => {
                code == "superseded-generation" || code == "retention-full"
            },
            _ => false,
        }
    }

    /// Validates a reply `error` object before admission: the numeric
    /// code is required, and a -32000 contract error requires its
    /// string code and reason. Standard transport errors carry no
    /// data requirements.
    /// Validates a reply `error` object before admission. The numeric
    /// code is required, and the JSON-RPC message is required text;
    /// a -32000 contract error additionally requires its string code
    /// and reason, with an optional details object (absent allowed,
    /// present non-object refused). Standard transport errors carry
    /// no data requirements.
    fn error_shape_valid(error: &Value) -> bool {
        let code = error.get("code").and_then(Value::as_i64);
        if error.get("message").and_then(Value::as_str).is_none() {
            return false;
        }
        match code {
            None => false,
            Some(-32000) => {
                let data = error.get("data");
                let details_ok = data
                    .and_then(|data| data.get("details"))
                    .is_none_or(Value::is_object);
                matches!(
                    (
                        data.and_then(|data| data.get("code"))
                            .and_then(Value::as_str),
                        data.and_then(|data| data.get("reason"))
                            .and_then(Value::as_str),
                        details_ok,
                    ),
                    (Some(_), Some(_), true)
                )
            },
            Some(_) => true,
        }
    }

    /// Resolves one admitted persist reply. A malformed outcome keeps
    /// the call AND its tracker entry open for a later valid reply;
    /// a valid terminal retires both. An uncertain write is never
    /// recorded as persisted without commit proof.
    pub(crate) fn resolve_persist(&mut self, envelope: &Value, wire: u64) {
        let is_error = envelope.get("error").is_some();
        if is_error {
            // Admitted errors (correlation_invalid by exemption, or a
            // standard transport error by table match) resolve the
            // write as dead without commit proof: never persisted.
            self.record("persist_error", "admitted error ends the write");
            if let Some(write_id) = self.persist_wires.remove(&wire) {
                self.persist.retire(&write_id);
            }
            self.table.retire(Direction::Outbound, wire);
            return;
        }
        let result = envelope.get("result").cloned().unwrap_or(Value::Null);
        if !Self::persist_result_valid(&result) {
            self.record(
                "persist_outcome_malformed",
                "uncertain write kept open",
            );
            return;
        }
        let persisted = result
            .get("persisted")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !persisted {
            self.record("persist_refused", "write not committed");
        }
        if let Some(write_id) = self.persist_wires.remove(&wire) {
            self.persist.retire(&write_id);
        }
        self.table.retire(Direction::Outbound, wire);
    }
}
