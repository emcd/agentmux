pub mod client;

pub use client::{AcpPromptCompletion, AcpRequestError, AcpRequestResult, AcpStdioClient};

pub const PROTOCOL_VERSION: u32 = 1;

/// A single entry captured during ACP session replay.
#[derive(Debug, Clone)]
pub enum ReplayEntry {
    /// User message text lines.
    User(Vec<String>),
    /// Agent response text lines.
    Agent(Vec<String>),
    /// Agent internal reasoning (thought chunk).
    Thinking(Vec<String>),
    /// Tool call invocation (title, status).
    ToolCall { title: String, status: String },
    /// Tool call result content.
    ToolResult(Vec<String>),
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
