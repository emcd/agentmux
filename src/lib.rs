//! Shared primitives for tmuxmux executables.

/// Returns a human-readable startup line for a binary.
pub fn startup_line(binary: &str) -> String {
    format!(
        "{binary} starting ({name} v{version})",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
    )
}

#[cfg(test)]
mod tests {
    use super::startup_line;

    #[test]
    fn startup_line_includes_binary_name() {
        let line = startup_line("tmuxmux-relay");
        assert!(line.contains("tmuxmux-relay"));
    }
}
