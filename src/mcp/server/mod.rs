//! MCP server surface for agentmux.
//!
//! Pure directory module. This file is an operator-hub: it only declares
//! submodules and re-exports the public surface (`McpConfiguration`,
//! `McpServer`, `run`). All other types (`McpState`, the shared relay
//! helpers, the `#[tool_handler]` impl) live in `service.rs`; the per-tool
//! `#[tool_router]` impl blocks live in `handlers/`.

mod handlers;
mod service;

pub use service::McpConfiguration;
pub(crate) use service::McpServer;
pub use service::run;
