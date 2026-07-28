//! MCP server surface for agentmux.

mod errors;
mod help;
mod params;
mod server;
mod validation;

pub use server::{McpConfiguration, McpReadiness, McpStartupFault, run};
