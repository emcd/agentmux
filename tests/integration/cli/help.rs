use std::process::Command;

#[test]
fn unified_host_help_output_includes_relay_and_mcp_modes() {
    let relay = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--help"])
        .output()
        .expect("run agentmux host relay --help");
    assert!(relay.status.success(), "relay help should succeed");
    let relay_stdout = String::from_utf8_lossy(&relay.stdout);
    assert!(
        relay_stdout.contains("Usage: agentmux host relay"),
        "unexpected relay help output: {relay_stdout}"
    );
    assert!(
        !relay_stdout.contains("--group GROUP"),
        "unexpected relay help output: {relay_stdout}"
    );
    assert!(
        relay_stdout.contains("--no-autostart"),
        "unexpected relay help output: {relay_stdout}"
    );

    let mcp = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "mcp", "--help"])
        .output()
        .expect("run agentmux host mcp --help");
    assert!(mcp.status.success(), "mcp help should succeed");
    let mcp_stdout = String::from_utf8_lossy(&mcp.stdout);
    assert!(
        mcp_stdout.contains("Usage: agentmux host mcp"),
        "unexpected mcp help output: {mcp_stdout}"
    );
}

#[test]
fn tui_help_output_includes_usage_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["tui", "--help"])
        .output()
        .expect("run agentmux tui --help");
    assert!(output.status.success(), "tui help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: agentmux tui"),
        "unexpected tui help output: {stdout}"
    );
    assert!(
        stdout.contains("--bundle NAME"),
        "unexpected tui help output: {stdout}"
    );
}

#[test]
fn list_help_output_includes_sessions_subcommand() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["list", "--help"])
        .output()
        .expect("run agentmux list --help");
    assert!(output.status.success(), "list help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: agentmux list sessions"),
        "unexpected list help output: {stdout}"
    );
}

#[test]
fn bare_agentmux_without_tty_prints_help_and_fails() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .output()
        .expect("run bare agentmux");
    assert!(
        !output.status.success(),
        "bare command should fail without tty"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("Usage: agentmux <command>"),
        "unexpected stdout: {stdout}"
    );
    assert!(
        stdout.contains("list sessions [--bundle NAME|--all] [--as-session NAME]"),
        "top-level help should advertise relocked list sessions surface: {stdout}"
    );
    assert!(
        !stdout.contains("\\n"),
        "help output should render line breaks, not literal escapes: {stdout}"
    );
    assert!(
        stderr.contains("validation_missing_subcommand"),
        "unexpected stderr: {stderr}"
    );
}
