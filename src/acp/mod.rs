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

/// Provenance marker for `ReplayEntry::User` entries. The replay buffer
/// carries user-authored content from two distinct sources:
///
/// - `PromptPath` — the operator's local submission, added synchronously
///   by `AcpStdioClient::prompt` before the agent response arrives.
/// - `ReaderThread` — chunks parsed from `session/update` or `session/load`
///   notifications (a `user_message_chunk` emitted by the upstream ACP
///   server as part of the session history).
///
/// These two sources MUST NOT coalesce: a prompt-origin `User` tail and a
/// reader-origin `User` arrival represent two distinct operator
/// submissions and must remain two buffer entries. Coalescence within a
/// source (e.g., multiple back-to-back `user_message_chunk`s from the
/// server) is allowed and applied by `try_merge_adjacent`; coalescence
/// across sources is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserSource {
    PromptPath,
    ReaderThread,
}

#[derive(Debug, Clone)]
pub enum ReplayEntry {
    User {
        lines: Vec<String>,
        source: UserSource,
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

/// Buffer-aware pending-tool-call record. The reader thread holds one of these
/// per in-flight `tool_call` notification so a later `tool_call_update` can
/// mutate the original buffer entry in place rather than appending a second
/// Invocation entry. `buffer_position` is the index of the Pending entry in
/// the buffer at the moment the parser pushed it; the
/// `enforce_replay_buffer_cap_and_maintain_positions` helper keeps it valid
/// across cap-driven drain.
///
/// Visibility is `pub` because the parser test re-exports in this module
/// carry `PendingToolCall` in their signatures. Internal production code
/// outside `crate::acp` should not depend on the fields; treat the type as
/// crate-private to `crate::acp` and rely on the test re-exports for any
/// cross-crate use.
#[derive(Debug, Clone)]
pub struct PendingToolCall {
    pub entry: ReplayEntry,
    pub buffer_position: usize,
}

pub fn parse_replay_entries_for_test(
    params: &Value,
    pending_calls: &mut HashMap<String, PendingToolCall>,
    buffer: &mut Vec<ReplayEntry>,
) {
    client::parse_replay_entries_from_params(params, pending_calls, buffer);
}

pub fn append_replay_entries_for_test(
    buffer: &mut Vec<ReplayEntry>,
    pending_calls: &mut HashMap<String, PendingToolCall>,
    entries: Vec<ReplayEntry>,
) {
    client::append_replay_entries(buffer, pending_calls, entries);
}

pub fn coalesce_replay_entries_on_append_for_test(
    buffer: &mut Vec<ReplayEntry>,
    entries: Vec<ReplayEntry>,
) {
    client::coalesce_replay_entries_on_append(buffer, entries);
}

pub fn enforce_replay_buffer_cap_and_maintain_positions_for_test(
    buffer: &mut Vec<ReplayEntry>,
    pending_calls: &mut HashMap<String, PendingToolCall>,
) {
    client::enforce_replay_buffer_cap_and_maintain_positions(buffer, pending_calls);
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
