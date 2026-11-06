//! Canonical Agentmux tool operations: the public, protocol-neutral
//! boundary for tool semantics.
//!
//! One typed Rust operation per operation-backed MCP tool. An operation
//! takes the public runtime handle plus relay-verified principal
//! context and a typed input, and returns a typed output or the
//! canonical [`OperationError`]. Calling one requires no MCP server,
//! stdio transport, rmcp router, or agent harness; no operation
//! signature names an MCP, rmcp, JSON-RPC, stdio, or JSON-schema type,
//! and no input carries a caller-supplied sender identity.
//!
//! The MCP adapter owns framing, schemas, association, and error
//! rendering, and maps its parameter types into these inputs
//! (`crate::mcp::operation_mapping`). `help` has no operation: its
//! subject matter is the adapter catalog and generated schemas.

pub mod error;
pub mod inputs;
pub mod outputs;

pub use self::error::OperationError;
pub use self::inputs::{
    BundleScope, ChangePskInput, ChangeScopeInput, DecideInput, DecideOutcome,
    DownInput, DropPeerInput, LinkPeerInput, ListDecisionsInput,
    ListNamespacesInput, ListPrincipalsInput, ListRelaysInput, LookInput,
    NewPeerInput, RawwInput, SendInput, UpInput,
};
pub use self::outputs::{
    BundleTransitionOutput, ChangePskOutput, ChangeScopeOutput, DecideOutput,
    DropPeerOutput, LinkPeerOutput, ListDecisionsOutput, ListNamespacesOutput,
    ListPrincipalsOutput, ListRelaysOutput, LookOutput, NewPeerOutput,
    RawwOutput, SendOutput,
};
