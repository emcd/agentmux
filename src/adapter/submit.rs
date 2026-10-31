//! `adapter.submit` execution and ack trichotomy.
//!
//! Single-flight turn gate with `RefusedBusy` as positive no-effect
//! evidence naming adapter single-flight; terminal outcomes use the
//! contract literals (`Submitted` / `TransportUnavailable` /
//! `SubmissionUnknown`) with `evidence_basis` on unknown and no-effect
//! outcomes. The gate limits turns, never Look/control progress.

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
