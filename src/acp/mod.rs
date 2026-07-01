pub mod client;
pub mod permission;
pub mod render;
pub mod state;
pub mod transport;
pub mod worker_driver;

pub use client::{
    AcpRequestError, AcpStdioClient, DispatchHandler, PermissionHandler, PermissionResponder,
    PromptCompletion, PromptCompletionHandler, PromptDispatchOutcome, REPLAY_BUFFER_MAX_ENTRIES,
};
pub use render::{replay_entries_to_snapshot_entries, snapshot_entries_to_plain_lines};
pub use transport::{
    ACP_ERROR_CODE_CONNECTION_CLOSED, ACP_ERROR_CODE_INITIALIZE_FAILED,
    ACP_ERROR_CODE_PROMPT_FAILED, ACP_ERROR_CODE_TRANSPORT_UNAVAILABLE, AcpBootstrapError,
    AcpTransport, PersistentAcpWorkerRuntime, bootstrap_acp_worker_runtime,
};
pub use worker_driver::{AcpDriverServices, AcpWorkerDriver};

use serde_json::Value;

use crate::transports::ToolCallStatus;

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

use std::collections::HashMap;

pub fn parse_replay_entries_for_test(
    params: &Value,
    pending_calls: &mut HashMap<String, ReplayEntry>,
) -> Vec<ReplayEntry> {
    client::parse_replay_entries_from_params(params, pending_calls)
}

pub fn append_replay_entries_for_test(buffer: &mut Vec<ReplayEntry>, entries: Vec<ReplayEntry>) {
    client::append_replay_entries(buffer, entries);
}

pub fn coalesce_replay_entries_on_append_for_test(
    buffer: &mut Vec<ReplayEntry>,
    entries: Vec<ReplayEntry>,
) {
    client::coalesce_replay_entries_on_append(buffer, entries);
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub request_id: u64,
    pub tool_call_title: String,
    pub requested_kind: String,
    pub requested_details: Value,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}
