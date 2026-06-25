//! Shared delivery and look vocabulary for the transport layer.
//!
//! These enums are the canonical wire/behavior vocabulary every transport and
//! the relay delivery path speak: the per-target delivery outcome, the payload
//! handling mode, the look-snapshot freshness/source markers, the structured
//! transcript entry type, and the transport-level look snapshot payload. They
//! live here — below `crate::relay` in the dependency order, importing only
//! `serde`/`serde_json` and no concrete transport module — so concrete
//! transports (`src/acp`, `src/tmux`) and the [`contract`](super::contract)
//! types can use them without a transport->relay back-edge and without the look
//! vocabulary reaching up into a concrete transport for its leaf type. The relay
//! re-exports them from its own contract module, so
//! `crate::relay::{SendOutcome, ...}` keeps resolving for existing relay and wire
//! consumers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
pub enum LookFreshness {
    Fresh,
    Stale,
}

/// Source marker for ACP-backed look snapshot responses.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LookSnapshotSource {
    LiveBuffer,
    None,
}

/// Readiness state for a persistent worker serving a delivery target.
///
/// Transport-agnostic: any worker-driven transport populates this state. ACP is
/// the only populator today; Pty is the next expected one.
///
/// The relay's per-target delivery workers mutate this state in an in-process
/// registry. Out-of-crate observers read it via
/// [`crate::relay::subscribe_worker_readiness`] — today only the ACP integration
/// tests; the relay's own respawn/startup gating reads it internally. The TUI
/// does **not** consume this enum: it observes worker transitions as relay wire
/// stream events instead. The state is stringified by the relay rather than
/// serialized directly, so it carries no `serde` derive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerReadinessState {
    Initializing,
    Available,
    Busy,
    Recovering,
    Unavailable,
}

/// Status of a tool-call invocation within a [`StructuredEntry::Invocation`].
///
/// Serialized without `rename_all`, so the wire spells these `Pending`/
/// `Completed` — preserved verbatim from when this type lived in `src/acp`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolCallStatus {
    Pending,
    Completed,
}

/// One entry in a structured agent transcript snapshot.
///
/// Transport-neutral: the `kind` set (`user`/`agent`/`cognition`/`invocation`/
/// `update`) describes a structured agent transcript, not ACP wire framing, and
/// `Invocation`'s `call_id`/`status`/`result` are general tool-use semantics.
/// ACP produces this from its own `ReplayEntry` intermediate, which stays
/// ACP-local.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StructuredEntry {
    User {
        lines: Vec<String>,
    },
    Agent {
        lines: Vec<String>,
    },
    Cognition {
        lines: Vec<String>,
    },
    Invocation {
        call_id: String,
        status: ToolCallStatus,
        invocation: Value,
        result: Option<Value>,
    },
    Update {
        update_kind: String,
        lines: Vec<String>,
    },
}

/// Transport-level snapshot payload returned by
/// [`OutputView::look`](super::contract::OutputView::look).
///
/// The structured variant carries the freshness metadata the relay forwards onto
/// its wire `LookSnapshotPayload`. Format-agnostic: the relay owns the wire
/// discriminator separately.
#[derive(Clone, Debug)]
pub enum LookSnapshotPayload {
    /// Plain text lines (tmux).
    Lines { snapshot_lines: Vec<String> },
    /// Rendered structured transcript entries plus truncation bookkeeping and
    /// freshness (ACP today).
    StructuredEntries {
        snapshot_entries: Vec<StructuredEntry>,
        /// Total entries available before tail/offset windowing.
        entries_total: usize,
        /// Count actually returned after the tail-N window and `offset`.
        returned_entries_count: usize,
        /// Whether the snapshot is fresh or stale.
        freshness: LookFreshness,
        /// Where the snapshot was sourced from.
        snapshot_source: LookSnapshotSource,
        /// Why the snapshot is stale, when applicable.
        stale_reason_code: Option<String>,
        /// Age of the snapshot in milliseconds, when known.
        snapshot_age_ms: Option<u64>,
    },
}
