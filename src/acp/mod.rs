pub mod client;
pub mod render;

pub use client::{AcpPromptCompletion, AcpRequestError, AcpRequestResult, AcpStdioClient};
pub use render::{
    AcpSnapshotEntry, replay_entries_to_snapshot_entries, snapshot_entries_to_plain_lines,
};

use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub enum ReplayEntry {
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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ToolCallStatus {
    Pending,
    Completed,
}

use std::collections::HashMap;

pub fn parse_replay_entries_for_test(
    params: &Value,
    pending_calls: &mut HashMap<String, ReplayEntry>,
    next_fallback_call_id: &mut u64,
) -> Vec<ReplayEntry> {
    client::parse_replay_entries_from_params(params, pending_calls, next_fallback_call_id)
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: u64,
    pub tool_call_title: String,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}
