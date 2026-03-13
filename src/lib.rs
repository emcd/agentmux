//! Shared primitives for agentmux executables.

pub mod commands;
pub mod configuration;
pub mod envelope;
pub mod mcp;
pub mod relay;
pub mod runtime;
pub mod tui;

/// Returns a human-readable startup line for a binary.
pub fn startup_line(binary: &str) -> String {
    format!(
        "{binary} starting ({name} v{version})",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    )
}
