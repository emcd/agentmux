//! Bootstrap-scenario integration tests for the runtime startup and MCP host
//! entry points. Two coherent surfaces:
//!
//! - [`mocks`]: `FakeRelay` (Unix-socket stub that records requests and
//!   answers them with a caller-supplied responder), `McpHarness` (a real
//!   `agentmux host mcp` subprocess with JSON-RPC `initialize`/send/read
//!   primitives), and three shared test fixtures (`write_bundle_configuration`,
//!   `write_bundle_configuration_with_directories`, `decode_tool_payload`).
//! - [`tests`]: the bootstrap scenarios themselves — concurrent bootstrap
//!   races for the relay socket, stale-socket removal, MCP startup without an
//!   active bundle context, explicit unknown-bundle startup failure,
//!   association from the injected bring-up environment, session resolution by
//!   declared member directories, retained startup faults that still serve the
//!   protocol, and reaching the relay of a named state directory.

mod mocks;
mod tests;
