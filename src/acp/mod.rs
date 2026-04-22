pub mod client;
pub mod render;

pub use client::{AcpPromptCompletion, AcpRequestError, AcpRequestResult, AcpStdioClient};
pub use render::{
    AcpSnapshotEntry, replay_entries_to_snapshot_entries, snapshot_entries_to_plain_lines,
};

use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;

/// A single entry captured during ACP session replay.
#[derive(Debug, Clone)]
pub enum ReplayEntry {
    /// User message text lines.
    User { lines: Vec<String> },
    /// Agent response text lines.
    Agent { lines: Vec<String> },
    /// Agent internal reasoning (thought chunk).
    Cognition { lines: Vec<String> },
    /// Tool call invocation payload.
    Invocation { invocation: Value },
    /// Tool call result payload.
    Result { result: Value },
    /// Fallback update payload for unknown/unsupported update kinds.
    Update {
        update_kind: String,
        lines: Vec<String>,
    },
}

/// A permission request from the ACP agent.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// JSON-RPC request ID for the response.
    pub request_id: u64,
    /// Tool call title (if available).
    pub tool_call_title: String,
    /// Available permission options.
    pub options: Vec<PermissionOption>,
}

/// A single permission option offered by the agent.
#[derive(Debug, Clone)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}
