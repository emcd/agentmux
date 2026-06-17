//! Shared delivery and look vocabulary for the transport layer.
//!
//! These enums are the canonical wire/behavior vocabulary every transport and
//! the relay delivery path speak: the per-target delivery outcome, the payload
//! handling mode, and the ACP look-snapshot freshness/source markers. They live
//! here — below `crate::relay` in the dependency order — so concrete transports
//! (`src/acp`, `src/tmux`) and the [`contract`](super::contract) types can use
//! them without a transport->relay back-edge. The relay re-exports them from its
//! own contract module, so `crate::relay::{SendOutcome, ...}` keeps resolving for
//! existing relay and wire consumers.

use serde::{Deserialize, Serialize};

/// Per-target delivery outcome for `send`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SendOutcome {
    Queued,
    Delivered,
    Timeout,
    DroppedOnShutdown,
    Failed,
}

/// Payload handling mode for one async delivery task.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryPayloadMode {
    EnvelopeMessage,
    RawInput,
}

/// Freshness status for ACP-backed look snapshot responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpLookFreshness {
    Fresh,
    Stale,
}

/// Source marker for ACP-backed look snapshot responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcpLookSnapshotSource {
    LiveBuffer,
    None,
}

/// Readiness state for a persistent ACP worker.
///
/// The relay's per-target delivery workers mutate this state in an in-process
/// registry. Out-of-crate observers read it via
/// [`crate::relay::subscribe_acp_worker_state`] — today only the ACP integration
/// tests; the relay's own respawn/startup gating reads it internally. The TUI
/// does **not** consume this enum: it observes worker transitions as relay wire
/// stream events instead. The state is stringified by the relay rather than
/// serialized directly, so it carries no `serde` derive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpWorkerReadinessState {
    Initializing,
    Available,
    Busy,
    Recovering,
    Unavailable,
}
