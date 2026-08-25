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
fn raww_help_output_includes_usage_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["raww", "--help"])
        .output()
        .expect("run agentmux raww --help");
    assert!(output.status.success(), "raww help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: agentmux raww"),
        "unexpected raww help output: {stdout}"
    );
    assert!(
        stdout.contains("--text TEXT"),
        "unexpected raww help output: {stdout}"
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
        stdout.contains("Usage: agentmux list principals"),
        "unexpected list help output: {stdout}"
    );
}

#[test]
fn version_flag_prints_crate_version_and_succeeds() {
    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .arg(flag)
            .output()
            .unwrap_or_else(|err| panic!("run agentmux {flag}: {err}"));
        assert!(output.status.success(), "{flag} should succeed: {output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let expected = format!("agentmux {}", env!("CARGO_PKG_VERSION"));
        assert_eq!(
            stdout.trim(),
            expected,
            "unexpected {flag} output: {stdout}"
        );
    }
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
        stdout.contains("list principals [--namespace NAME|GLOBAL|*] [--as-session NAME]"),
        "top-level help should advertise relocked list principals surface: {stdout}"
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

#[test]
fn drop_help_output_includes_usage_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["drop", "--help"])
        .output()
        .expect("run agentmux drop --help");
    assert!(output.status.success(), "drop help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage: agentmux drop peer <principal_id>"),
        "unexpected drop help output: {stdout}"
    );
}

#[test]
fn drop_appears_in_the_top_level_command_topology() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["--help"])
        .output()
        .expect("run agentmux --help");
    assert!(output.status.success(), "help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("drop peer <principal_id>"),
        "drop must be listed alongside its sibling admin commands: {stdout}"
    );
}

#[test]
fn drop_without_a_subcommand_is_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["drop"])
        .output()
        .expect("run agentmux drop");
    assert!(
        !output.status.success(),
        "drop with no subcommand must not succeed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing drop subcommand"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn drop_peer_without_a_principal_id_is_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["drop", "peer"])
        .output()
        .expect("run agentmux drop peer");
    assert!(
        !output.status.success(),
        "drop peer with no principal_id must not succeed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("drop peer requires a <principal_id> argument"),
        "unexpected stderr: {stderr}"
    );
}
