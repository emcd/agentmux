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

/// The dispatch verbs `run_agentmux` matches on, which the help topology must
/// name exactly. Kept as an explicit roster because Rust cannot enumerate match
/// arms at runtime: adding a subcommand and its help line fails this test until
/// the roster is updated, which is the point — the update is where a developer
/// notices the surface grew.
const DISPATCHED_SUBCOMMANDS: [&str; 12] = [
    "host", "up", "down", "list", "look", "raww", "new", "change", "drop", "check", "tui", "send",
];

/// Extracts the leading token of each entry under the help output's `Commands:`
/// heading, which is the verb `run_agentmux` dispatches on.
fn help_topology_subcommands(stdout: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_commands = false;
    for line in stdout.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        // The section ends at the first blank line; `Global flags:` follows.
        if line.trim().is_empty() {
            break;
        }
        if let Some(name) = line
            .split_whitespace()
            .next()
            .filter(|name| !names.iter().any(|existing| existing == name))
        {
            names.push(name.to_string());
        }
    }
    names
}

#[test]
fn help_topology_lists_exactly_the_dispatched_subcommands() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["--help"])
        .output()
        .expect("run agentmux --help");
    assert!(output.status.success(), "help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut listed = help_topology_subcommands(&stdout);
    listed.sort_unstable();
    let mut expected: Vec<String> = DISPATCHED_SUBCOMMANDS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    expected.sort_unstable();
    assert_eq!(
        listed, expected,
        "the help topology must name exactly the dispatched subcommands; \
         a dispatchable command missing here is undiscoverable, and a listed \
         one that does not dispatch is a broken promise: {stdout}"
    );
}

#[test]
fn every_subcommand_named_in_the_help_topology_dispatches() {
    // `host` and `tui` are excluded: both start long-running work rather than
    // failing fast on missing arguments, so invoking them here would hang. Their
    // presence is covered by the exact-set assertion above and by their own
    // command-surface tests.
    for name in DISPATCHED_SUBCOMMANDS
        .iter()
        .filter(|name| !matches!(**name, "host" | "tui"))
    {
        let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
            .args([name])
            .output()
            .unwrap_or_else(|source| panic!("run agentmux {name}: {source}"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("unknown subcommand"),
            "'{name}' is named in the help topology but does not dispatch: {stderr}"
        );
    }
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
