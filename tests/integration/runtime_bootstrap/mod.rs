//! Bootstrap-scenario integration tests for the runtime startup and MCP host
//! entry points. Two coherent surfaces:
//!
//! - [`mocks`]: `FakeRelay` (Unix-socket stub that records requests and
//!   answers them with a caller-supplied responder), `McpHarness` (a real
//!   `agentmux host mcp` subprocess with JSON-RPC `initialize`/send/read
//!   primitives), and four shared test fixtures (`write_bundle_configuration`,
//!   `write_bundle_configuration_with_directories`, `decode_tool_payload`,
//!   `hook_git_environment`).
//! - [`tests`]: the 7 bootstrap scenarios themselves — concurrent
//!   bootstrap races for the relay socket, stale-socket removal, MCP
//!   startup without an active bundle context, explicit unknown-bundle
//!   startup failure, association discovery from a non-git cwd, directory
//!   fallback when the auto-sender is not a configured member, and the
//!   debug-build repository-root socket override.

mod mocks;
mod tests;
