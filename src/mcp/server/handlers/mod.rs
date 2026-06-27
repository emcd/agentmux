//! Per-tool `#[tool_router]` impl blocks for the MCP server. One file per
//! tool: list, help, send, look, raww, choose, updown, new, change. Each file
//! holds an `impl McpServer` block with a named `#[tool_router]` attribute
//! that generates a router-builder function; the routers are composed in
//! `McpServer::new()` (in `service.rs`).

mod change;
mod choose;
mod help;
mod list;
mod look;
mod new;
mod raww;
mod send;
mod updown;
