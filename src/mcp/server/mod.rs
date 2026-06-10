//! MCP server surface for agentmux.
//!
//! Pure directory module. This file is an operator-hub: it only declares
//! submodules and re-exports the public surface (`McpConfiguration`,
//! `McpServer`, `run`). All other types (`McpState`, the shared relay
//! helpers, the `#[tool_handler]` impl) live in `core.rs`; the per-tool
//! `#[tool_router]` impl blocks live in `handlers/`.

mod core;
mod handlers;

pub use core::McpConfiguration;
pub(crate) use core::McpServer;
pub use core::run;
