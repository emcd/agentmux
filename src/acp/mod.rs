pub mod client;

pub use client::{AcpPromptCompletion, AcpRequestError, AcpRequestResult, AcpStdioClient};

pub const PROTOCOL_VERSION: u32 = 1;
