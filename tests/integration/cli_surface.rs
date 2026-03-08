use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn host_relay_requires_bundle_positional_argument() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay"])
        .output()
        .expect("run agentmux host relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid argument <bundle-id>: missing value"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_missing_message_input() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo"])
        .stdin(Stdio::null())
        .output()
        .expect("run agentmux send without message");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_missing_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn send_rejects_conflicting_flag_and_piped_message_sources() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["send", "--target", "bravo", "--message", "hello"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentmux send");
    {
        let stdin = child.stdin.as_mut().expect("open child stdin");
        stdin
            .write_all(b"hello from stdin")
            .expect("write piped input");
    }
    let output = child.wait_with_output().expect("wait for child");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_conflicting_message_input"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn legacy_wrappers_still_support_help_output() {
    let relay = Command::new(env!("CARGO_BIN_EXE_agentmux-relay"))
        .arg("--help")
        .output()
        .expect("run agentmux-relay --help");
    assert!(relay.status.success(), "relay help should succeed");
    let relay_stdout = String::from_utf8_lossy(&relay.stdout);
    assert!(
        relay_stdout.contains("Usage: agentmux-relay"),
        "unexpected relay help output: {relay_stdout}"
    );

    let mcp = Command::new(env!("CARGO_BIN_EXE_agentmux-mcp"))
        .arg("--help")
        .output()
        .expect("run agentmux-mcp --help");
    assert!(mcp.status.success(), "mcp help should succeed");
    let mcp_stdout = String::from_utf8_lossy(&mcp.stdout);
    assert!(
        mcp_stdout.contains("Usage: agentmux-mcp"),
        "unexpected mcp help output: {mcp_stdout}"
    );
}
