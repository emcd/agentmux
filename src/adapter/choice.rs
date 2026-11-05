//! `adapter.requestChoice` raising and originated-call discipline.
//!
//! Generation-bound requests with originator-monotonic wire ids from 1,
//! pending-table admission, resolver blocking limited to the adapter's
//! own turn, dual-cancel on disconnect, record-before-respond, and the
//! negotiated in-flight cap with typed local shed (inscribed, never
//! queued). Owns the pending-call table for calls the adapter
//! originates (table storage lives in [`binding`](crate::adapter::binding)).

/// Resolution of a raised choice: chosen option or cancellation with
/// a namespaced reason code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChoiceResolution {
    Chosen {
        option_id: String,
        decided_by: String,
    },
    Cancelled {
        decided_by: Option<String>,
        reason_code: String,
    },
}

/// Resolver for one raised choice. Records the outcome before answering
/// the agent: `resolve` and `cancel` return the recorded outcome, and
/// repeated calls observe the same recorded value.
#[derive(Debug)]
pub struct ChoiceResolver {
    resolution: Option<ChoiceResolution>,
}

impl ChoiceResolver {
    /// Creates a pending resolver.
    #[must_use]
    pub fn new() -> Self {
        Self { resolution: None }
    }

    /// Records a decision and returns the recorded outcome.
    #[must_use]
    pub fn resolve(
        &mut self,
        resolution: ChoiceResolution,
    ) -> &ChoiceResolution {
        self.resolution.get_or_insert(resolution)
    }

    /// Records a cancellation and returns the recorded outcome.
    /// Cancelling twice keeps the first outcome: the turn observes one
    /// terminal, never a replacement.
    #[must_use]
    pub fn cancel(&mut self, reason_code: &str) -> &ChoiceResolution {
        self.resolution.get_or_insert(ChoiceResolution::Cancelled {
            decided_by: None,
            reason_code: reason_code.to_string(),
        })
    }

    /// Reports whether a terminal outcome is recorded.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.resolution.is_some()
    }
}

impl Default for ChoiceResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// One structured record for the local diagnostic sink (shed
/// gate-drops, cleanup records). Never stdout: diagnostics must not
/// corrupt the frame stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticEntry {
    pub code: String,
    pub detail: String,
}

/// Sink for diagnostic records. The live adapter writes to a
/// structured log / fixture-visible record; tests substitute a plain
/// vector.
pub trait DiagnosticSink {
    /// Records one diagnostic entry.
    fn record(&mut self, entry: DiagnosticEntry);
}

impl DiagnosticSink for Vec<DiagnosticEntry> {
    fn record(&mut self, entry: DiagnosticEntry) {
        self.push(entry);
    }
}

/// Admission decision for a further originated choice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateDecision {
    Admit,
    Shed { code: String },
}

/// In-flight cap for originated choices. A choice need arriving past
/// the negotiated cap sheds locally with a typed internal refusal,
/// inscribed with its code — never queued unboundedly.
#[derive(Debug)]
pub struct ChoiceGate {
    max_outstanding: usize,
    outstanding: usize,
}

impl ChoiceGate {
    /// Creates a gate allowing `max_outstanding` concurrent choices.
    #[must_use]
    pub fn new(max_outstanding: usize) -> Self {
        Self {
            max_outstanding,
            outstanding: 0,
        }
    }

    /// Admits a choice need or sheds it, inscribing the shed.
    pub fn request(
        &mut self,
        sink: &mut (impl DiagnosticSink + ?Sized),
    ) -> GateDecision {
        if self.outstanding >= self.max_outstanding {
            let code = "adapter_single_flight_busy".to_string();
            sink.record(DiagnosticEntry {
                code: code.clone(),
                detail: "choice shed at negotiated in-flight cap".to_string(),
            });
            return GateDecision::Shed { code };
        }
        self.outstanding = self.outstanding.saturating_add(1);
        GateDecision::Admit
    }

    /// Releases one outstanding choice on its terminal outcome.
    pub fn release(&mut self) {
        self.outstanding = self.outstanding.saturating_sub(1);
    }

    /// Releases all outstanding choices. Terminalization calls this
    /// after recording each open choice's terminal outcome.
    pub fn drain(&mut self) {
        self.outstanding = 0;
    }

    /// Counts outstanding choices.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }
}

use serde_json::Value;

use super::Adapter;
use super::binding::Direction;
use super::submit::OutcomeLiteral;

impl Adapter<'_> {
    /// Resolves one admitted choice reply. Only shape-valid replies
    /// reach this point (malformed results are discarded with the
    /// call kept open before admission), so every branch below
    /// records a terminal for the pending turn: a valid Chosen or
    /// Cancelled outcome from a result, or a local Cancelled carrying
    /// the admitted error's reason. The turn completes with its
    /// scripted ack in all three cases.
    pub(crate) fn resolve_choice(&mut self, envelope: &Value, wire: u64) {
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
        if let Some(result) = envelope.get("result") {
            // Shape-valid by the admission gate: exactly one of
            // chosen or cancelled with well-typed fields.
            let outcome = result.get("outcome");
            let chosen = outcome.and_then(|outcome| outcome.get("chosen"));
            let cancelled =
                outcome.and_then(|outcome| outcome.get("cancelled"));
            if let Some(chosen) = chosen {
                let option_id = chosen
                    .get("option_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let decided_by = chosen
                    .get("decided_by")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let _ = resolver.resolve(ChoiceResolution::Chosen {
                    option_id: option_id.to_string(),
                    decided_by: decided_by.to_string(),
                });
            } else if let Some(cancelled) = cancelled {
                let reason_code = cancelled
                    .get("reason_code")
                    .and_then(Value::as_str)
                    .unwrap_or("relay_cancelled");
                let decided_by = cancelled
                    .get("decided_by")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let _ = resolver.resolve(ChoiceResolution::Cancelled {
                    decided_by,
                    reason_code: reason_code.to_string(),
                });
            }
        } else if let Some(error) = envelope.get("error") {
            let reason = error
                .get("data")
                .and_then(|data| data.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("relay_error")
                .to_string();
            let _ = resolver.cancel(&reason);
        }
        self.choice_gate.release();
        let turn = self.config.script.turn_for(&pending.correlation);
        let ack = turn
            .map(|turn| turn.ack.as_outcome())
            .unwrap_or(OutcomeLiteral::Submitted);
        self.table.retire(Direction::Outbound, wire);
        let id = Value::String(pending.inbound_wire.to_string());
        self.complete_submit(
            &id,
            pending.inbound_wire,
            &pending.correlation,
            ack,
        );
    }
}
