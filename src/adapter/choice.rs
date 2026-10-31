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
    pub fn request(&mut self, sink: &mut impl DiagnosticSink) -> GateDecision {
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

    /// Counts outstanding choices.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }
}
