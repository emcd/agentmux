//! Direct public-consumer tests for canonical tool operations.
//!
//! Each module exercises one operation through the public runtime and
//! operations APIs only: no MCP server, no stdio transport, no rmcp
//! router, no agent harness. Compiled only under
//! `experimental-public-send`, matching the gated surface exercised.

#[cfg(feature = "experimental-public-send")]
mod send;
