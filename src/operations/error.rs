//! Canonical operation error: code, message, and typed details.
//!
//! The set is adapter-independent: relay-authored denials (including
//! `authorization_forbidden` and the dispatch admission refusals) enter
//! unchanged via [`OperationError::from_relay`], while adapter-only
//! failures (`validation_unassociated_server`, socket-transport
//! `relay_timeout` / `relay_unavailable`) never enter it. Details ride
//! as structured JSON so relay-authored detail shapes pass through
//! without the operation layer maintaining a parallel taxonomy per
//! code. The `link` adapter mapping is the documented exception that
//! drops details at render time for secret redaction; the canonical
//! error still carries them.

use serde_json::Value;

use crate::relay::RelayError;

/// Canonical failure of a tool operation.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationError {
    /// Stable machine-readable code (`validation_*`, `authorization_*`,
    /// `runtime_*`, `internal_*`).
    pub code: String,
    /// Human-readable summary.
    pub message: String,
    /// Structured detail payload, when the failure carries one.
    pub details: Option<Value>,
}

impl OperationError {
    /// Builds a canonical error from its parts.
    pub fn new(code: &str, message: &str, details: Option<Value>) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
            details,
        }
    }

    /// Lossless lift of a relay-authored denial: code and details are
    /// preserved unchanged.
    pub fn from_relay(error: &RelayError) -> Self {
        Self {
            code: error.code.clone(),
            message: error.message.clone(),
            details: error.details.clone(),
        }
    }
}

impl From<RelayError> for OperationError {
    fn from(error: RelayError) -> Self {
        Self::from_relay(&error)
    }
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for OperationError {}
