//! ACP stdio client: spawns the agent child, frames JSON-RPC over
//! stdio, and pumps the reader thread.
//!
//! Mechanical split of the former `client.rs` (1029 lines): `types` holds
//! the shared outcome enums, shared-state aliases, and supervision
//! handles; `protocol` owns [`AcpStdioClient`] and its request/prompt
//! surface; `reader` pumps stdout into replay, permission, and response
//! dispatch. Public paths through `crate::acp::client` are unchanged.

mod protocol;
mod reader;
mod types;

pub use protocol::AcpStdioClient;
pub(in crate::acp) use protocol::write_line_to_stdin;
pub(crate) use types::SharedReplay;
pub(in crate::acp) use types::SharedStdin;
pub use types::{
    AcpGenerationHandle, AcpRequestError, DispatchHandler, PromptCompletion,
    PromptCompletionHandler, PromptDispatchOutcome,
};
