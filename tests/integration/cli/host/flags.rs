//! CLI argument rejection for positional bundle selectors and unknown flag
//! combinations on the `host relay` subcommand.

use std::process::Command;

#[test]
fn host_relay_rejects_positional_bundle_selector() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "alpha"])
        .output()
        .expect("run agentmux host relay");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validation_invalid_arguments"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn host_relay_rejects_group_selector_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_agentmux"))
        .args(["host", "relay", "--group", "dev"])
        .output()
        .expect("run agentmux host relay with group selector");
    assert!(!output.status.success(), "command should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--group") && stderr.contains("unknown argument"),
        "unexpected stderr: {stderr}"
    );
}
