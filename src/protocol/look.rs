//! Look-snapshot vocabulary: freshness and source markers, the structured
//! transcript entry type, and the transport-level snapshot payload.
//!
//! `look` is the relay-to-transport call direction. Its vocabulary sits in this
//! boundary alongside the transport-to-relay mailbox vocabulary so neither
//! direction has to name a type the other owns.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// Transport-level snapshot payload returned by a transport's `OutputView::look`
/// implementation.
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
