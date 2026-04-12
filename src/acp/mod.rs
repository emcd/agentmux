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
